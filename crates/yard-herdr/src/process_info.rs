//! Herdr `pane.process_info`: the foreground process group of one pane.
//!
//! Herdr 0.9.1 answers `{"method":"pane.process_info","params":{"pane_id":..}}`
//! (CLI: `herdr pane process-info --pane <id>`) with a `pane_process_info`
//! result whose `foreground_processes` are the members of the pane shell's
//! foreground process group, each with `pid`, `name` and, when readable,
//! `argv`/`cmdline`.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::json;
use tokio::task::JoinSet;
use yard_domain::{ForegroundProcess, PaneForegroundJob};

use crate::{HerdrConfig, HerdrError, socket::request_command};

/// At most this many `pane.process_info` requests are in flight at once.
/// Herdr serves one request per connection, so a batch is concurrent
/// connections rather than one pipelined stream.
pub const MAX_CONCURRENT_PROCESS_INFO_REQUESTS: usize = 8;

/// Timeout of one `pane.process_info` request. Shorter than the general
/// request timeout: inspection runs inline in every inventory call, so a
/// stalled pane must not hold the snapshot for long.
pub const PROCESS_INFO_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

/// No new batch of inspections starts after this much time; the remaining
/// panes are reported as timed out. With [`PROCESS_INFO_REQUEST_TIMEOUT`]
/// the whole inspection phase is bounded by the sum of both.
pub const PROCESS_INFO_PHASE_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize)]
struct ProcessInfoResult {
    process_info: ProcessInfo,
}

#[derive(Debug, Deserialize)]
struct ProcessInfo {
    pane_id: String,
    #[serde(default)]
    foreground_process_group_id: Option<u32>,
    #[serde(default)]
    foreground_processes: Vec<Process>,
}

#[derive(Debug, Deserialize)]
struct Process {
    pid: u32,
    name: String,
    #[serde(default)]
    argv: Option<Vec<String>>,
}

pub(crate) fn decode_process_info(
    result: serde_json::Value,
    pane_id: &str,
) -> Result<PaneForegroundJob, HerdrError> {
    let actual = result
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("missing")
        .to_owned();
    if actual != "pane_process_info" {
        return Err(HerdrError::UnexpectedResult {
            expected: "pane_process_info",
            actual,
        });
    }
    let result: ProcessInfoResult =
        serde_json::from_value(result).map_err(HerdrError::CommandDecode)?;
    let info = result.process_info;
    if info.pane_id != pane_id {
        return Err(HerdrError::UnexpectedResult {
            expected: "pane_process_info for the requested pane",
            actual: format!("pane_process_info for '{}'", info.pane_id),
        });
    }
    Ok(PaneForegroundJob {
        pane_id: info.pane_id,
        process_group_id: info.foreground_process_group_id,
        processes: info
            .foreground_processes
            .into_iter()
            .map(|process| ForegroundProcess {
                pid: process.pid,
                name: process.name,
                argv: process.argv,
            })
            .collect(),
    })
}

pub(crate) async fn pane_process_info(
    config: &HerdrConfig,
    socket_path: &Path,
    pane_id: &str,
) -> Result<PaneForegroundJob, HerdrError> {
    let result = request_command(
        config,
        socket_path,
        &format!("yard:process-info:{pane_id}"),
        "pane.process_info",
        json!({ "pane_id": pane_id }),
        config.request_timeout.min(PROCESS_INFO_REQUEST_TIMEOUT),
    )
    .await?;
    decode_process_info(result, pane_id)
}

/// Inspect several panes with bounded concurrency. The result keeps the
/// requested order; one failed pane never fails the others. Panes whose batch
/// would start after [`PROCESS_INFO_PHASE_DEADLINE`] are not requested and
/// are reported as [`HerdrError::SocketTimeout`].
pub(crate) async fn pane_process_infos(
    config: &HerdrConfig,
    socket_path: &Path,
    pane_ids: Vec<String>,
) -> Vec<(String, Result<PaneForegroundJob, HerdrError>)> {
    pane_process_infos_until(
        config,
        socket_path,
        pane_ids,
        Instant::now() + PROCESS_INFO_PHASE_DEADLINE,
    )
    .await
}

async fn pane_process_infos_until(
    config: &HerdrConfig,
    socket_path: &Path,
    pane_ids: Vec<String>,
    deadline: Instant,
) -> Vec<(String, Result<PaneForegroundJob, HerdrError>)> {
    let config = Arc::new(config.clone());
    let socket_path: Arc<Path> = Arc::from(socket_path);
    let mut results = Vec::with_capacity(pane_ids.len());
    for chunk in pane_ids.chunks(MAX_CONCURRENT_PROCESS_INFO_REQUESTS) {
        if Instant::now() >= deadline {
            results.extend(
                chunk
                    .iter()
                    .map(|pane_id| (pane_id.clone(), Err(HerdrError::SocketTimeout))),
            );
            continue;
        }
        let mut tasks = JoinSet::new();
        for (index, pane_id) in chunk.iter().cloned().enumerate() {
            let config = Arc::clone(&config);
            let socket_path = Arc::clone(&socket_path);
            tasks.spawn(async move {
                let result = pane_process_info(&config, &socket_path, &pane_id).await;
                (index, pane_id, result)
            });
        }
        let mut chunk_results = Vec::with_capacity(chunk.len());
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(result) => chunk_results.push(result),
                Err(error) => tracing::warn!(%error, "Herdr process-info task failed"),
            }
        }
        chunk_results.sort_by_key(|(index, _, _)| *index);
        results.extend(
            chunk_results
                .into_iter()
                .map(|(_, pane_id, result)| (pane_id, result)),
        );
    }
    results
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };

    use super::{decode_process_info, pane_process_infos, pane_process_infos_until};
    use crate::{HerdrConfig, HerdrError};

    const ID: &str = "01a0b10f-4a75-7841-8e1d-aa6f12919c45";

    /// A Herdr 0.9.1 `pane.process_info` result as returned on the socket.
    fn result(pane_id: &str) -> serde_json::Value {
        json!({
            "type": "pane_process_info",
            "process_info": {
                "pane_id": pane_id,
                "shell_pid": 100,
                "foreground_process_group_id": 200,
                "foreground_processes": [
                    {
                        "pid": 200,
                        "name": "codex",
                        "argv": ["codex", "resume", ID],
                        "cmdline": format!("codex resume {ID}"),
                        "cwd": "/home/u/project"
                    },
                    { "pid": 201, "name": "kworker" }
                ]
            }
        })
    }

    #[test]
    fn decodes_herdr_0_9_1_process_info() {
        let job = decode_process_info(result("w1:pB"), "w1:pB").unwrap();

        assert_eq!(job.pane_id, "w1:pB");
        assert_eq!(job.process_group_id, Some(200));
        assert_eq!(job.processes.len(), 2);
        assert_eq!(job.processes[0].pid, 200);
        assert_eq!(
            job.processes[0].argv.as_deref(),
            Some(&["codex".to_owned(), "resume".to_owned(), ID.to_owned()][..])
        );
        assert_eq!(job.processes[1].argv, None);
    }

    #[test]
    fn decodes_a_pane_without_foreground_job() {
        let job = decode_process_info(
            json!({"type":"pane_process_info","process_info":{"pane_id":"w1:p1"}}),
            "w1:p1",
        )
        .unwrap();

        assert_eq!(job.process_group_id, None);
        assert!(job.processes.is_empty());
    }

    #[test]
    fn rejects_a_result_for_another_pane_or_type() {
        assert!(matches!(
            decode_process_info(result("w1:pC"), "w1:pB"),
            Err(HerdrError::UnexpectedResult { .. })
        ));
        assert!(matches!(
            decode_process_info(json!({"type":"pane_info"}), "w1:pB"),
            Err(HerdrError::UnexpectedResult { .. })
        ));
    }

    #[tokio::test]
    async fn inspects_each_pane_over_its_own_connection() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let mut methods = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let pane_id = request["params"]["pane_id"].as_str().unwrap().to_owned();
                methods.push((
                    request["method"].as_str().unwrap().to_owned(),
                    pane_id.clone(),
                ));
                let response = if pane_id == "w1:pB" {
                    json!({"id": request["id"], "result": result(&pane_id)})
                } else {
                    json!({"id": request["id"], "error": {"code":"pane_not_found","message":"pane not found"}})
                };
                let mut bytes = serde_json::to_vec(&response).unwrap();
                bytes.push(b'\n');
                writer.write_all(&bytes).await.unwrap();
            }
            methods
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(1),
            ..HerdrConfig::default()
        };

        let results = pane_process_infos(
            &config,
            &socket_path,
            vec!["w1:pB".to_owned(), "w1:p9".to_owned()],
        )
        .await;
        let mut methods = server.await.unwrap();
        methods.sort();

        assert_eq!(
            methods,
            vec![
                ("pane.process_info".to_owned(), "w1:p9".to_owned()),
                ("pane.process_info".to_owned(), "w1:pB".to_owned()),
            ]
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "w1:pB");
        assert_eq!(results[0].1.as_ref().unwrap().process_group_id, Some(200));
        assert_eq!(results[1].0, "w1:p9");
        assert!(matches!(results[1].1, Err(HerdrError::Api { .. })));
    }

    #[tokio::test]
    async fn a_stalled_pane_is_bounded_by_the_short_timeout_and_the_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        // Accepts and reads, but never answers.
        let server = tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                held.push(stream);
            }
        });
        let config = HerdrConfig {
            request_timeout: Duration::from_secs(5),
            ..HerdrConfig::default()
        };

        let started = std::time::Instant::now();
        let stalled = pane_process_infos(&config, &socket_path, vec!["w1:pB".to_owned()]).await;
        let elapsed = started.elapsed();
        assert!(matches!(stalled[0].1, Err(HerdrError::SocketTimeout)));
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");

        // Past the deadline no request is sent; every pane times out at once.
        let started = std::time::Instant::now();
        let panes: Vec<String> = (0..20).map(|index| format!("w1:p{index}")).collect();
        let late = pane_process_infos_until(
            &config,
            &socket_path,
            panes.clone(),
            std::time::Instant::now(),
        )
        .await;
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(
            late.iter()
                .map(|(pane, _)| pane.clone())
                .collect::<Vec<_>>(),
            panes
        );
        assert!(
            late.iter()
                .all(|(_, result)| matches!(result, Err(HerdrError::SocketTimeout)))
        );
        server.abort();
    }
}
