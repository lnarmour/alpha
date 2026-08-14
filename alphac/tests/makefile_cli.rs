//! `alphac --makefile` CLI-level behavior (issue #22, sub-issue of #21): the generated `Makefile`
//! is written next to `-o`'s own path, `--makefile` without `-o` is a hard error (same reasoning as
//! `--wrapper`, see `wrapper_cli.rs`), and — combined with `--wrapper` — the resulting Makefile
//! actually builds a working executable via `make`.

use std::path::PathBuf;
use std::process::Command;

fn alphac_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_alphac"))
}

fn copy_input_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../tests/alpha-language-fixtures/alpha.codegen.tests/resources/CopyInput/CopyInput.alpha",
    )
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "alphac_makefile_cli_test_{tag}_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("creating temp output dir");
    dir
}

#[test]
fn makefile_flag_writes_a_makefile_next_to_the_output_path() {
    let dir = temp_dir("plain");
    let output_path = dir.join("copyinput.c");

    let status = Command::new(alphac_bin())
        .arg(copy_input_fixture())
        .arg("-o")
        .arg(&output_path)
        .arg("--makefile")
        .status()
        .expect("running alphac");
    assert!(status.success(), "alphac --makefile exited non-zero");

    let makefile_path = dir.join("Makefile");
    assert!(
        makefile_path.exists(),
        "expected a Makefile at {makefile_path:?}, next to the -o path, but it wasn't created"
    );
    let makefile_src = std::fs::read_to_string(&makefile_path).expect("reading Makefile");
    assert!(makefile_src.contains("copyinput.o: copyinput.c"));
    assert!(
        !makefile_src.contains("_wrapper:"),
        "no --wrapper was given, so the Makefile shouldn't link any wrapper executable"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn makefile_flag_without_output_path_is_a_hard_error() {
    let output = Command::new(alphac_bin())
        .arg(copy_input_fixture())
        .arg("--makefile")
        .output()
        .expect("running alphac");
    assert!(
        !output.status.success(),
        "alphac --makefile with no -o should fail, not succeed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--makefile requires -o"),
        "expected a clear error about --makefile needing -o, got: {stderr}"
    );
}

#[test]
fn makefile_and_wrapper_together_produce_a_working_executable_via_make() {
    let dir = temp_dir("wrapper");
    let output_path = dir.join("copyinput.c");

    let status = Command::new(alphac_bin())
        .arg(copy_input_fixture())
        .arg("-o")
        .arg(&output_path)
        .arg("--wrapper")
        .arg("--makefile")
        .status()
        .expect("running alphac");
    assert!(
        status.success(),
        "alphac --wrapper --makefile exited non-zero"
    );

    let makefile_path = dir.join("Makefile");
    let wrapper_path = dir.join("copyinput_wrapper.c");
    assert!(makefile_path.exists(), "Makefile was not created");
    assert!(wrapper_path.exists(), "wrapper file was not created");
    let makefile_src = std::fs::read_to_string(&makefile_path).expect("reading Makefile");
    assert!(makefile_src.contains("copyinput_wrapper: copyinput_wrapper.c copyinput.o"));

    let make_status = Command::new("make")
        .arg("-C")
        .arg(&dir)
        .status()
        .expect("running make — a C compiler + make are required to build this workspace");
    assert!(make_status.success(), "make failed to build the Makefile");

    let bin_path = dir.join("copyinput_wrapper");
    assert!(bin_path.exists(), "make did not produce the wrapper binary");

    let run_status = Command::new(&bin_path)
        .arg("7")
        .status()
        .expect("running the compiled wrapper binary");
    assert!(
        run_status.success(),
        "the Makefile-built wrapper binary did not run successfully"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
