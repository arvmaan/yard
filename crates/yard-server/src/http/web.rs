use std::path::Path;

use axum::{
    body::{Body, Bytes},
    extract::Request,
    http::{HeaderValue, Method, StatusCode, header},
    response::Response,
};
use include_dir::{Dir, include_dir};
use percent_encoding::percent_decode_str;

static WEB_DIST: Dir<'static> = include_dir!("$OUT_DIR/web");

const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
const CACHE_REVALIDATE: &str = "no-cache";
const CACHE_NONE: &str = "no-store";

pub(super) async fn serve(request: Request) -> Response {
    let method = request.method();
    if method != Method::GET && method != Method::HEAD {
        return empty_response(StatusCode::NOT_FOUND);
    }

    let Ok(path) = decode_path(request.uri().path()) else {
        return empty_response(StatusCode::BAD_REQUEST);
    };
    if is_server_namespace(&path) {
        return empty_response(StatusCode::NOT_FOUND);
    }

    if let Some(file) = WEB_DIST.get_file(&path) {
        return file_response(file.path(), file.contents(), method == Method::HEAD);
    }

    if is_asset_path(&path) {
        return empty_response(StatusCode::NOT_FOUND);
    }

    let Some(index) = WEB_DIST.get_file("index.html") else {
        return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
    };
    file_response(index.path(), index.contents(), method == Method::HEAD)
}

fn decode_path(raw_path: &str) -> Result<String, ()> {
    validate_percent_encoding(raw_path)?;
    let decoded = percent_decode_str(raw_path).decode_utf8().map_err(|_| ())?;
    let relative = decoded.strip_prefix('/').ok_or(())?;
    if relative.contains('\\') || relative.chars().any(char::is_control) {
        return Err(());
    }

    let mut segments = relative.split('/').peekable();
    while let Some(segment) = segments.next() {
        if segment == "." || segment == ".." || (segment.is_empty() && segments.peek().is_some()) {
            return Err(());
        }
    }
    Ok(relative.to_owned())
}

fn validate_percent_encoding(path: &str) -> Result<(), ()> {
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(encoded) = bytes.get(index + 1..index + 3) else {
                return Err(());
            };
            if !encoded.iter().all(u8::is_ascii_hexdigit) {
                return Err(());
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn is_server_namespace(path: &str) -> bool {
    path == "api" || path.starts_with("api/") || path == "health" || path.starts_with("health/")
}

fn is_asset_path(path: &str) -> bool {
    path.starts_with("assets/") || Path::new(path).extension().is_some()
}

fn file_response(path: &Path, bytes: &'static [u8], head_only: bool) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let content_type = match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => mime.as_ref(),
    };
    let cache_control = if path.starts_with("assets") {
        CACHE_IMMUTABLE
    } else {
        CACHE_REVALIDATE
    };
    let body = if head_only {
        Body::empty()
    } else {
        Body::from(Bytes::from_static(bytes))
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(header::CACHE_CONTROL, cache_control)
        .header(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        )
        .body(body)
        .expect("static embedded response headers must be valid")
}

fn empty_response(status: StatusCode) -> Response {
    Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, CACHE_NONE)
        .header(header::CONTENT_LENGTH, 0)
        .header(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        )
        .body(Body::empty())
        .expect("static empty response headers must be valid")
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::{CACHE_IMMUTABLE, CACHE_NONE, CACHE_REVALIDATE, WEB_DIST, serve};

    fn app() -> Router {
        Router::new().fallback(serve)
    }

    async fn request(method: Method, uri: &str) -> axum::response::Response {
        app()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    fn fingerprinted_asset(extension: &str) -> String {
        WEB_DIST
            .get_dir("assets")
            .expect("Vite must emit an assets directory")
            .files()
            .find_map(|file| {
                let path = file.path().to_str()?;
                let matches_extension = file
                    .path()
                    .extension()
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(extension));
                (path.starts_with("assets/") && matches_extension).then(|| path.to_owned())
            })
            .unwrap_or_else(|| panic!("Vite must emit a fingerprinted {extension} asset"))
    }

    #[tokio::test]
    async fn serves_root_html_without_immutable_caching() {
        let response = request(Method::GET, "/").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], CACHE_REVALIDATE);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            body.windows(15)
                .any(|window| window == b"<div id=\"root\">")
        );
    }

    #[tokio::test]
    async fn serves_fingerprinted_assets_with_mime_and_immutable_caching() {
        for (extension, content_type) in [
            ("css", "text/css; charset=utf-8"),
            ("js", "text/javascript; charset=utf-8"),
            ("woff", "font/woff"),
            ("woff2", "font/woff2"),
        ] {
            let asset = fingerprinted_asset(extension);
            let response = request(Method::GET, &format!("/{asset}")).await;

            assert_eq!(response.status(), StatusCode::OK, "{asset}");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                content_type,
                "{asset}"
            );
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                CACHE_IMMUTABLE,
                "{asset}"
            );
            assert_ne!(response.headers()[header::CONTENT_LENGTH], "0", "{asset}");
        }
    }

    #[tokio::test]
    async fn falls_back_to_uncached_index_for_client_routes() {
        let response = request(Method::GET, "/projects/example/overview").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], CACHE_REVALIDATE);
    }

    #[tokio::test]
    async fn supports_head_without_sending_embedded_bytes() {
        let asset = fingerprinted_asset("js");
        let response = request(Method::HEAD, &format!("/{asset}")).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_ne!(response.headers()[header::CONTENT_LENGTH], "0");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn does_not_mask_server_namespaces_or_non_get_methods() {
        for (method, uri) in [
            (Method::GET, "/api/v1/missing"),
            (Method::GET, "/api%2fv1%2fmissing"),
            (Method::GET, "/health/missing"),
            (Method::POST, "/client-route"),
        ] {
            let response = request(method, uri).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], CACHE_NONE);
            assert!(response.headers().get(header::CONTENT_TYPE).is_none());
        }
    }

    #[tokio::test]
    async fn rejects_traversal_and_malformed_asset_paths() {
        for uri in [
            "/../Cargo.toml",
            "/%2e%2e/Cargo.toml",
            "/assets/%2e%2e/index.html",
            "/assets/%ZZ.js",
            "/assets/%00.js",
            "/assets/%0A.js",
            "/assets/%FF.js",
            "/assets//index.js",
        ] {
            let response = request(Method::GET, uri).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], CACHE_NONE);
        }
    }

    #[tokio::test]
    async fn missing_asset_paths_remain_not_found() {
        for uri in ["/assets/missing.js", "/favicon.ico"] {
            let response = request(Method::GET, uri).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert!(response.headers().get(header::CONTENT_TYPE).is_none());
        }
    }
}
