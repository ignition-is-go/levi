use std::fs;
use std::path::PathBuf;

fn workspace_file(path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("levi crate is in workspace root")
        .to_path_buf();
    fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

#[test]
fn action_installs_verified_release_assets_and_runs_the_gate() {
    let action = workspace_file("action.yml");
    for expected in [
        "x86_64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "levi-${version}-${target}.tgz",
        "$archive.sha256",
        "refs/levi/events:refs/levi/events",
        "check-claims --git-ref",
        "ACTION_PATH/levi/Cargo.toml",
    ] {
        assert!(action.contains(expected), "action.yml missing {expected}");
    }
    assert!(action.contains("Checksum mismatch"));
    assert!(action.contains("inputs.fetch-events == 'true'"));
    assert!(action.contains("inputs.check == 'true'"));
}

#[test]
fn release_builds_checksums_and_publishes_before_smoke_testing() {
    let workflow = workspace_file(".github/workflows/release.yml");
    for expected in [
        "release-binaries:",
        "cargo build --release --locked --package levi",
        "actions/upload-artifact@v4",
        "actions/download-artifact@v4",
        "sha256sum --check",
        "gh release create",
        "verify-action:",
        "uses: ./",
    ] {
        assert!(
            workflow.contains(expected),
            "release workflow missing {expected}"
        );
    }
    let publish = workflow.find("publish-release:").unwrap();
    let verify = workflow.find("verify-action:").unwrap();
    assert!(
        publish < verify,
        "action smoke test must follow release publication"
    );
}
