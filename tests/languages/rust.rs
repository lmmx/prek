use anyhow::Result;
use assert_fs::assert::PathAssert;
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

/// Test `language_version` parsing and installation for Rust hooks.
#[test]
fn language_version() -> Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other rust versions installed locally.
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rust-system
                name: rust-system
                language: rust
                entry: rustc --version
                language_version: system
                pass_filenames: false
                always_run: true
              - id: rust-1.70 # should auto install 1.70.X
                name: rust-1.70
                language: rust
                entry: rustc --version
                language_version: '1.70'
                always_run: true
                pass_filenames: false
              - id: rust-1.70 # run again to ensure reusing the installed version
                name: rust-1.70
                language: rust
                entry: rustc --version
                language_version: '1.70'
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    // Toolchains should be in the shared rustup home
    let rustup_dir = context.home_dir().child("tools").child("rustup");

    let filters = [
        (r"rustc (1\.70)\.\d{1,2} .+", "rustc $1.X"), // Keep 1.70.X format
        (r"rustc 1\.\d{1,3}\.\d{1,2} .+", "rustc 1.X.X"), // Others become 1.X.X
    ]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-system..............................................................Passed
    - hook id: rust-system
    - duration: [TIME]

      rustc 1.X.X
    rust-1.70................................................................Passed
    - hook id: rust-1.70
    - duration: [TIME]

      rustc 1.70.X
    rust-1.70................................................................Passed
    - hook id: rust-1.70
    - duration: [TIME]

      rustc 1.70.X

    ----- stderr -----
    "#);

    // Verify toolchains are in shared rustup home
    let toolchains_dir = rustup_dir.child("toolchains");
    toolchains_dir.assert(predicates::path::is_dir());

    // There should be at least one toolchain installed (1.70.x)
    let installed_toolchains = toolchains_dir
        .read_dir()?
        .flatten()
        .filter_map(|d| {
            let filename = d.file_name().to_string_lossy().to_string();
            if filename.starts_with('.') {
                None
            } else {
                Some(filename)
            }
        })
        .collect::<Vec<_>>();

    assert!(
        installed_toolchains.iter().any(|v| v.starts_with("1.70")),
        "Expected Rust 1.70.X toolchain to be installed, but found: {installed_toolchains:?}"
    );

    Ok(())
}

/// Test that multiple hooks with different Rust versions share the same rustup.
#[test]
fn shared_rustup() -> Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rust-stable
                name: rust-stable
                language: rust
                entry: rustc --version
                language_version: stable
                pass_filenames: false
                always_run: true
              - id: rust-1.70
                name: rust-1.70
                language: rust
                entry: rustc --version
                language_version: '1.70'
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    let filters = [
        (r"rustc (1\.70)\.\d{1,2} .+", "rustc $1.X"),
        (r"rustc 1\.\d{1,3}\.\d{1,2} .+", "rustc 1.X.X"),
    ]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-stable..............................................................Passed
    - hook id: rust-stable
    - duration: [TIME]

      rustc 1.X.X
    rust-1.70................................................................Passed
    - hook id: rust-1.70
    - duration: [TIME]

      rustc 1.70.X

    ----- stderr -----
    "#);

    // Both toolchains should be in the shared rustup home
    let rustup_dir = context.home_dir().child("tools").child("rustup");
    let toolchains_dir = rustup_dir.child("toolchains");
    toolchains_dir.assert(predicates::path::is_dir());

    let installed_toolchains = toolchains_dir
        .read_dir()?
        .flatten()
        .filter_map(|d| {
            let filename = d.file_name().to_string_lossy().to_string();
            if filename.starts_with('.') {
                None
            } else {
                Some(filename)
            }
        })
        .collect::<Vec<_>>();

    // Should have both stable and 1.70.x
    assert!(
        installed_toolchains.len() >= 2,
        "Expected at least 2 toolchains, found: {installed_toolchains:?}"
    );
    assert!(
        installed_toolchains.iter().any(|v| v.starts_with("stable")),
        "Expected stable toolchain, found: {installed_toolchains:?}"
    );
    assert!(
        installed_toolchains.iter().any(|v| v.starts_with("1.70")),
        "Expected 1.70.X toolchain, found: {installed_toolchains:?}"
    );

    // There should be only one rustup binary (in cargo home, not per-toolchain)
    let cargo_dir = context
        .home_dir()
        .child("tools")
        .child("rust")
        .child("cargo");
    let rustup_bin = cargo_dir.child("bin").child(if cfg!(windows) {
        "rustup.exe"
    } else {
        "rustup"
    });
    rustup_bin.assert(predicates::path::is_file());

    Ok(())
}

/// Test that `additional_dependencies` with cli: prefix are installed correctly.
#[test]
fn additional_dependencies_cli() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: rust-cli
                name: rust-cli
                language: rust
                entry: prek-rust-echo Hello, Prek!
                additional_dependencies: ["cli:prek-rust-echo"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-cli.................................................................Passed
    - hook id: rust-cli
    - duration: [TIME]

      Hello, Prek!

    ----- stderr -----
    ");
}

/// Test that remote Rust hooks are installed and run correctly.
#[test]
fn remote_hooks() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-test-repos/rust-hooks
            rev: v1.0.0
            hooks:
              - id: hello-world
                verbose: true
                pass_filenames: false
                always_run: true
                args: ["Hello World"]
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World..............................................................Passed
    - hook id: hello-world
    - duration: [TIME]

      Hello World

    ----- stderr -----
    ");
}

/// Test that library dependencies (non-cli: prefix) work correctly on remote hooks.
/// This verifies that the shared repo is not modified when adding dependencies.
#[test]
fn remote_hooks_with_lib_deps() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-test-repos/rust-hooks
            rev: v1.0.0
            hooks:
              - id: hello-world-lib-deps
                additional_dependencies: ["itoa:1"]
                verbose: true
                pass_filenames: false
                always_run: true
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World Lib Deps.....................................................Passed
    - hook id: hello-world-lib-deps
    - duration: [TIME]

      42

    ----- stderr -----
    ");
}

/// Test that system rustup is reused when available.
#[test]
fn reuse_system_rustup() {
    // This test verifies that when rustup is available on the system,
    // we use it instead of downloading a new one.
    // We can't easily test this in CI without mocking, but we can verify
    // the behavior works correctly.

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rust-stable
                name: rust-stable
                language: rust
                entry: rustc --version
                language_version: stable
                pass_filenames: false
                always_run: true
    "});
    context.git_add(".");

    let filters = [(r"rustc 1\.\d{1,3}\.\d{1,2} .+", "rustc 1.X.X")]
        .into_iter()
        .chain(context.filters())
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-stable..............................................................Passed
    - hook id: rust-stable
    - duration: [TIME]

      rustc 1.X.X

    ----- stderr -----
    "#);

    // Toolchains should be in the shared RUSTUP_HOME regardless of whether
    // we used system rustup or installed our own
    let rustup_dir = context.home_dir().child("tools").child("rustup");
    let toolchains_dir = rustup_dir.child("toolchains");
    toolchains_dir.assert(predicates::path::is_dir());
}
