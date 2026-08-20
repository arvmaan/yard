use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

const WEB_INPUTS: &[&str] = &[
    "index.html",
    "package.json",
    "package-lock.json",
    "playwright.config.ts",
    "public",
    "src",
    "tests",
    "tsconfig.json",
    "tsconfig.app.json",
    "tsconfig.node.json",
    "tsconfig.test.json",
    "vite.config.ts",
    "vitest.config.ts",
];

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR"),
    );
    let web_dir = manifest_dir.join("../../web");
    let output_dir =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR")).join("web");

    for input in WEB_INPUTS {
        println!("cargo::rerun-if-changed={}", web_dir.join(input).display());
    }

    if !web_dir.join("node_modules").is_dir() {
        fail(&format!(
            "frontend dependencies are missing at {}; run `cd web && npm ci` from the Yard \
             repository root, then rerun Cargo",
            web_dir.join("node_modules").display()
        ));
    }

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let status = Command::new(npm)
        .args(["run", "build", "--", "--outDir"])
        .arg(&output_dir)
        .arg("--emptyOutDir")
        .current_dir(&web_dir)
        .status()
        .unwrap_or_else(|error| {
            fail(&format!(
                "could not start npm ({error}); install a supported Node.js/npm version and run \
                 `cd {} && npm ci`",
                web_dir.display()
            ))
        });

    require_success(status, &web_dir);
    require_output(&output_dir);
}

fn require_success(status: ExitStatus, web_dir: &Path) {
    if !status.success() {
        fail(&format!(
            "frontend build failed with {status}; run `cd {} && npm ci && npm run build` to \
             diagnose it",
            web_dir.display()
        ));
    }
}

fn require_output(output_dir: &Path) {
    if !output_dir.join("index.html").is_file() {
        fail(&format!(
            "frontend build succeeded but did not create {}",
            output_dir.join("index.html").display()
        ));
    }
}

fn fail(message: &str) -> ! {
    panic!("cannot embed the Yard web app: {message}");
}
