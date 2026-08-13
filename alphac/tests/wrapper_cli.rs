//! `alphac --wrapper` CLI-level behavior (issue #23): the wrapper file's name/location is derived
//! from `-o`'s own path (not the input file's), and `--wrapper` without `-o` is a hard error since
//! there's no path to derive a name/location from.

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

#[test]
fn wrapper_flag_writes_a_wrapper_file_next_to_the_output_path() {
    let dir = std::env::temp_dir().join(format!("alphac_wrapper_cli_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating temp output dir");
    let output_path = dir.join("copyinput.c");

    let status = Command::new(alphac_bin())
        .arg(copy_input_fixture())
        .arg("-o")
        .arg(&output_path)
        .arg("--wrapper")
        .status()
        .expect("running alphac");
    assert!(status.success(), "alphac --wrapper exited non-zero");

    assert!(output_path.exists(), "main output file was not written");
    let wrapper_path = dir.join("copyinput_wrapper.c");
    assert!(
        wrapper_path.exists(),
        "expected wrapper file at {wrapper_path:?}, next to the -o path, but it wasn't created"
    );

    let wrapper_src = std::fs::read_to_string(&wrapper_path).expect("reading wrapper file");
    assert!(wrapper_src.contains("int main(int argc, char **argv)"));
    assert!(wrapper_src.contains("CopyInput"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wrapper_flag_without_output_path_is_a_hard_error() {
    let output = Command::new(alphac_bin())
        .arg(copy_input_fixture())
        .arg("--wrapper")
        .output()
        .expect("running alphac");
    assert!(
        !output.status.success(),
        "alphac --wrapper with no -o should fail, not succeed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--wrapper requires -o"),
        "expected a clear error about --wrapper needing -o, got: {stderr}"
    );
}
