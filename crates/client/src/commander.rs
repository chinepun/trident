use fehler::{throw, throws};
use std::path::PathBuf;
use std::process;
use std::{borrow::Cow, io, os::unix::process::CommandExt, process::Stdio, string::FromUtf8Error};
use thiserror::Error;
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
    signal,
};
use trident_fuzz::config::Config;

use crate::constants::*;
use tokio::io::AsyncBufReadExt;
use trident_fuzz::fuzz_stats::FuzzingStatistics;

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0:?}")]
    Io(#[from] io::Error),
    #[error("{0:?}")]
    Utf8(#[from] FromUtf8Error),
    #[error("localnet is not running")]
    LocalnetIsNotRunning,
    #[error("localnet is still running")]
    LocalnetIsStillRunning,
    #[error("build programs failed")]
    BuildProgramsFailed,
    #[error("testing failed")]
    TestingFailed,
    #[error("read program code failed: '{0}'")]
    ReadProgramCodeFailed(String),
    #[error("{0:?}")]
    TomlDeserialize(#[from] toml::de::Error),
    #[error("parsing Cargo.toml dependencies failed")]
    ParsingCargoTomlDependenciesFailed,
    #[error("fuzzing failed")]
    FuzzingFailed,
    #[error("Trident it not correctly initialized! The trident-tests folder in the root of your project does not exist")]
    NotInitialized,
    #[error("the crash file does not exist")]
    CrashFileNotFound,
    #[error("The Solana project does not contain any programs")]
    NoProgramsFound,
}

/// `Commander` allows you to start localnet, build programs,
/// run tests and do other useful operations.
pub struct Commander {
    root: Cow<'static, str>,
}

impl Commander {
    /// Creates a new `Commander` instance with the default root `"../../"`.
    pub fn new() -> Self {
        Self {
            root: "../../".into(),
        }
    }

    /// Creates a new `Commander` instance with the provided `root`.
    pub fn with_root(root: impl Into<Cow<'static, str>>) -> Self {
        Self { root: root.into() }
    }

    #[throws]
    pub async fn build_anchor_project(&self) {
        let success = Command::new("anchor")
            .arg("build")
            .spawn()?
            .wait()
            .await?
            .success();
        if !success {
            throw!(Error::BuildProgramsFailed);
        }
    }
    /// Runs fuzzer on the given target with exit code option.
    #[throws]
    pub async fn run_fuzzer_with_exit_code(&self, target: String) {
        let config = Config::new();

        // obtain hfuzz_run_args from env variable, this variable can contain multiple
        // arguments so we need to parse the variable content.
        let hfuzz_run_args = std::env::var("HFUZZ_RUN_ARGS").unwrap_or_default();

        let genesis_folder = PathBuf::from(self.root.to_string()).join("trident-genesis");

        let mut fuzz_args = config.get_honggfuzz_args(hfuzz_run_args);

        // let cargo_target_dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_default();

        // obtain cargo_target_dir, as this variable contains only 1 string
        // which corresponds to desired path, we can compare it to the Config
        // the default/desired value is set inside Config, however variable entered
        // form CLI has always precedence
        let cargo_target_dir = std::env::var("CARGO_TARGET_DIR")
            .unwrap_or_else(|_| config.get_env_arg("CARGO_TARGET_DIR"));

        // obtain hfuzz_workspace, as this variable contains only 1 string
        // which corresponds to desired path, we can compare it to the Config
        // the default/desired value is set inside Config, however variable entered
        // form CLI has always precedence
        let hfuzz_workspace = std::env::var("HFUZZ_WORKSPACE")
            .unwrap_or_else(|_| config.get_env_arg("HFUZZ_WORKSPACE"));

        let (crash_dir, ext) =
            get_crash_dir_and_ext(&self.root, &target, &fuzz_args, &hfuzz_workspace);

        if let Ok(crash_files) = get_crash_files(&crash_dir, &ext) {
            if !crash_files.is_empty() {
                println!("{ERROR} The crash directory {} already contains crash files from previous runs. \n\nTo run Trident fuzzer with exit code, you must either (backup and) remove the old crash files or alternatively change the crash folder using for example the --crashdir option and the HFUZZ_RUN_ARGS env variable such as:\nHFUZZ_RUN_ARGS=\"--crashdir ./new_crash_dir\"", crash_dir.to_string_lossy());
                process::exit(1);
            }
        }

        match config.get_fuzzing_with_stats() {
            true => {
                // enforce keep output to be true
                fuzz_args.push_str("--keep_output");
                let mut child = Command::new("cargo")
                    .env("GENESIS_FOLDER", genesis_folder)
                    .env("HFUZZ_RUN_ARGS", fuzz_args)
                    .env("CARGO_TARGET_DIR", cargo_target_dir)
                    .env("HFUZZ_WORKSPACE", hfuzz_workspace)
                    .arg("hfuzz")
                    .arg("run")
                    .arg(target)
                    .stdout(Stdio::piped())
                    .spawn()?;
                Self::handle_child_with_stats(&mut child).await?;
            }
            false => {
                let mut child = Command::new("cargo")
                    .env("GENESIS_FOLDER", genesis_folder)
                    .env("HFUZZ_RUN_ARGS", fuzz_args)
                    .env("CARGO_TARGET_DIR", cargo_target_dir)
                    .env("HFUZZ_WORKSPACE", hfuzz_workspace)
                    .arg("hfuzz")
                    .arg("run")
                    .arg(target)
                    .spawn()?;
                Self::handle_child(&mut child).await?;
            }
        }

        if let Ok(crash_files) = get_crash_files(&crash_dir, &ext) {
            if !crash_files.is_empty() {
                println!(
                    "The crash directory {} contains new fuzz test crashes. Exiting!",
                    crash_dir.to_string_lossy()
                );
                process::exit(99);
            }
        }
    }

    /// Runs fuzzer on the given target.
    #[throws]
    pub async fn run_fuzzer(&self, target: String) {
        let config = Config::new();

        let hfuzz_run_args = std::env::var("HFUZZ_RUN_ARGS").unwrap_or_default();

        let genesis_folder = PathBuf::from(self.root.to_string()).join("trident-genesis");

        let cargo_target_dir = std::env::var("CARGO_TARGET_DIR")
            .unwrap_or_else(|_| config.get_env_arg("CARGO_TARGET_DIR"));
        let hfuzz_workspace = std::env::var("HFUZZ_WORKSPACE")
            .unwrap_or_else(|_| config.get_env_arg("HFUZZ_WORKSPACE"));

        let mut fuzz_args = config.get_honggfuzz_args(hfuzz_run_args);

        match config.get_fuzzing_with_stats() {
            true => {
                // enforce keep output to be true
                fuzz_args.push_str("--keep_output");
                let mut child = Command::new("cargo")
                    .env("GENESIS_FOLDER", genesis_folder)
                    .env("HFUZZ_RUN_ARGS", fuzz_args)
                    .env("CARGO_TARGET_DIR", cargo_target_dir)
                    .env("HFUZZ_WORKSPACE", hfuzz_workspace)
                    .arg("hfuzz")
                    .arg("run")
                    .arg(target)
                    .stdout(Stdio::piped())
                    .spawn()?;
                Self::handle_child_with_stats(&mut child).await?;
            }
            false => {
                let mut child = Command::new("cargo")
                    .env("GENESIS_FOLDER", genesis_folder)
                    .env("HFUZZ_RUN_ARGS", fuzz_args)
                    .env("CARGO_TARGET_DIR", cargo_target_dir)
                    .env("HFUZZ_WORKSPACE", hfuzz_workspace)
                    .arg("hfuzz")
                    .arg("run")
                    .arg(target)
                    .spawn()?;
                Self::handle_child(&mut child).await?;
            }
        }
    }

    /// Manages a child process in an async context, specifically for monitoring fuzzing tasks.
    /// Waits for the process to exit or a Ctrl+C signal. Prints an error message if the process
    /// exits with an error, and sleeps briefly on Ctrl+C. Throws `Error::FuzzingFailed` on errors.
    ///
    /// # Arguments
    /// * `child` - A mutable reference to a `Child` process.
    ///
    /// # Errors
    /// * Throws `Error::FuzzingFailed` if waiting on the child process fails.
    #[throws]
    async fn handle_child(child: &mut Child) {
        tokio::select! {
            res = child.wait() =>
                match res {
                    Ok(status) => if !status.success() {
                        throw!(Error::FuzzingFailed);
                    },
                    Err(_) => throw!(Error::FuzzingFailed),
            },
            _ = signal::ctrl_c() => {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            },
        }
    }
    /// Asynchronously manages a child fuzzing process, collecting and logging its statistics.
    /// This function spawns a new task dedicated to reading the process's standard output and logging the fuzzing statistics.
    /// It waits for either the child process to exit or a Ctrl+C signal to be received. Upon process exit or Ctrl+C signal,
    /// it stops the logging task and displays the collected statistics in a table format.
    ///
    /// The implementation ensures that the statistics logging task only stops after receiving a signal indicating the end of the fuzzing process
    /// or an interrupt from the user, preventing premature termination of the logging task if scenarios where reading is faster than fuzzing,
    /// which should not be common.
    ///
    /// # Arguments
    /// * `child` - A mutable reference to a `Child` process, representing the child fuzzing process.
    ///
    /// # Errors
    /// * `Error::FuzzingFailed` - Thrown if there's an issue with managing the child process, such as failing to wait on the child process.
    #[throws]
    async fn handle_child_with_stats(child: &mut Child) {
        let stdout = child
            .stdout
            .take()
            .expect("child did not have a handle to stdout");

        let reader = tokio::io::BufReader::new(stdout);

        let fuzz_end = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fuzz_end_clone = std::sync::Arc::clone(&fuzz_end);

        let stats_handle: tokio::task::JoinHandle<Result<FuzzingStatistics, std::io::Error>> =
            tokio::spawn(async move {
                let mut stats_logger = FuzzingStatistics::new();

                let mut lines = reader.lines();
                loop {
                    let _line = lines.next_line().await;
                    match _line {
                        Ok(__line) => match __line {
                            Some(content) => {
                                stats_logger.insert_serialized(&content);
                            }
                            None => {
                                if fuzz_end_clone.load(std::sync::atomic::Ordering::SeqCst) {
                                    break;
                                }
                            }
                        },
                        Err(e) => return Err(e),
                    }
                }
                Ok(stats_logger)
            });

        tokio::select! {
            res = child.wait() =>{
                fuzz_end.store(true, std::sync::atomic::Ordering::SeqCst);

                match res {
                    Ok(status) => {
                        if !status.success() {
                            throw!(Error::FuzzingFailed);
                        }
                    },
                    Err(_) => throw!(Error::FuzzingFailed),
                }
            },
            _ = signal::ctrl_c() => {
                fuzz_end.store(true, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            },
        }
        let stats_result = stats_handle
            .await
            .expect("Unable to obtain Statistics Handle");
        match stats_result {
            Ok(stats_result) => {
                stats_result.show_table();
            }
            Err(e) => {
                println!("Statistics thread exited with the Error: {}", e);
            }
        }
    }

    /// Runs fuzzer on the given target.
    #[throws]
    pub async fn run_fuzzer_debug(&self, target: String, crash_file_path: String) {
        let config = Config::new();

        let crash_file = std::path::Path::new(&self.root as &str).join(crash_file_path);

        let genesis_folder = PathBuf::from(self.root.to_string()).join("trident-genesis");

        if !crash_file.try_exists()? {
            println!("{ERROR} The crash file [{:?}] not found", crash_file);
            throw!(Error::CrashFileNotFound);
        }

        let cargo_target_dir = std::env::var("CARGO_TARGET_DIR")
            .unwrap_or_else(|_| config.get_env_arg("CARGO_TARGET_DIR"));

        // using exec rather than spawn and replacing current process to avoid unflushed terminal output after ctrl+c signal
        std::process::Command::new("cargo")
            .env("GENESIS_FOLDER", genesis_folder)
            .env("CARGO_TARGET_DIR", cargo_target_dir)
            .arg("hfuzz")
            .arg("run-debug")
            .arg(target)
            .arg(crash_file)
            .exec();

        eprintln!("cannot execute \"cargo hfuzz run-debug\" command");
    }
    pub fn program_packages() -> impl Iterator<Item = cargo_metadata::Package> {
        let cargo_toml_data = cargo_metadata::MetadataCommand::new()
            .no_deps()
            .exec()
            .expect("Cargo.toml reading failed");

        cargo_toml_data.packages.into_iter().filter(|package| {
            // TODO less error-prone test if the package is a _program_?
            if let Some("programs") = package.manifest_path.iter().nth_back(2) {
                return true;
            }
            false
        })
    }
    /// Collects and returns a list of program packages using cargo metadata.
    #[throws]
    pub async fn collect_program_packages() -> Vec<cargo_metadata::Package> {
        let packages: Vec<cargo_metadata::Package> = Commander::program_packages().collect();
        if packages.is_empty() {
            throw!(Error::NoProgramsFound)
        } else {
            packages
        }
    }

    /// Formats program code.
    #[throws]
    pub async fn format_program_code(code: &str) -> String {
        let mut rustfmt = Command::new("rustfmt")
            .args(["--edition", "2018"])
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        if let Some(stdio) = &mut rustfmt.stdin {
            stdio.write_all(code.as_bytes()).await?;
        }
        let output = rustfmt.wait_with_output().await?;
        String::from_utf8(output.stdout)?
    }

    /// Formats program code - nightly.
    #[throws]
    pub async fn format_program_code_nightly(code: &str) -> String {
        let mut rustfmt = Command::new("rustfmt")
            .arg("+nightly")
            .arg("--config")
            .arg("blank_lines_upper_bound=1,blank_lines_lower_bound=1")
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        if let Some(stdio) = &mut rustfmt.stdin {
            stdio.write_all(code.as_bytes()).await?;
        }
        let output = rustfmt.wait_with_output().await?;
        String::from_utf8(output.stdout)?
    }
}

impl Default for Commander {
    /// Creates a new `Commander` instance with the default root `"../../"`.
    fn default() -> Self {
        Self::new()
    }
}

fn get_crash_dir_and_ext(
    root: &str,
    target: &str,
    hfuzz_run_args: &str,
    hfuzz_workspace: &str,
) -> (PathBuf, String) {
    // FIXME: we split by whitespace without respecting escaping or quotes - same approach as honggfuzz-rs so there is no point to fix it here before the upstream is fixed
    let hfuzz_run_args = hfuzz_run_args.split_whitespace();

    let extension =
        get_cmd_option_value(hfuzz_run_args.clone(), "-e", "--ext").unwrap_or("fuzz".to_string());

    // If we run fuzzer like:
    // HFUZZ_WORKSPACE="./new_hfuzz_workspace" HFUZZ_RUN_ARGS="--crashdir ./new_crash_dir -W ./new_workspace" cargo hfuzz run
    // The structure will be as follows:
    // ./new_hfuzz_workspace - will contain inputs
    // ./new_crash_dir - will contain crashes
    // ./new_workspace - will contain report
    // So finally , we have to give precedence:
    // --crashdir > --workspace > HFUZZ_WORKSPACE
    let crash_dir = get_cmd_option_value(hfuzz_run_args.clone(), "", "--cr")
        .or_else(|| get_cmd_option_value(hfuzz_run_args.clone(), "-W", "--w"));

    let crash_path = if let Some(dir) = crash_dir {
        // INFO If path is absolute, it replaces the current path.
        std::path::Path::new(root).join(dir)
    } else {
        std::path::Path::new(hfuzz_workspace).join(target)
    };

    (crash_path, extension)
}

fn get_cmd_option_value<'a>(
    hfuzz_run_args: impl Iterator<Item = &'a str>,
    short_opt: &str,
    long_opt: &str,
) -> Option<String> {
    let mut args_iter = hfuzz_run_args;
    let mut value: Option<String> = None;

    // ensure short option starts with one dash and long option with two dashes
    let short_opt = format!("-{}", short_opt.trim_start_matches('-'));
    let long_opt = format!("--{}", long_opt.trim_start_matches('-'));

    while let Some(arg) = args_iter.next() {
        match arg.strip_prefix(&short_opt) {
            Some(val) if short_opt.len() > 1 => {
                if !val.is_empty() {
                    // -ecrash for crash extension with no space
                    value = Some(val.to_string());
                } else if let Some(next_arg) = args_iter.next() {
                    // -e crash for crash extension with space
                    value = Some(next_arg.to_string());
                } else {
                    value = None;
                }
            }
            _ => {
                if arg.starts_with(&long_opt) && long_opt.len() > 2 {
                    value = args_iter.next().map(|a| a.to_string());
                }
            }
        }
    }

    value
}

fn get_crash_files(
    dir: &PathBuf,
    extension: &str,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let paths = std::fs::read_dir(dir)?
        // Filter out all those directory entries which couldn't be read
        .filter_map(|res| res.ok())
        // Map the directory entries to paths
        .map(|dir_entry| dir_entry.path())
        // Filter out all paths with extensions other than `extension`
        .filter_map(|path| {
            if path.extension().map_or(false, |ext| ext == extension) {
                Some(path)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    pub const HFUZZ_WORKSPACE_DEFAULT: &str = "trident-tests/fuzz_tests/fuzzing/hfuzz_workspace";
    #[test]
    fn test_cmd_options_parsing() {
        let mut command = String::from("-Q -v --extension fuzz");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, Some("fuzz".to_string()));

        command = String::from("-Q --extension fuzz -v");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, Some("fuzz".to_string()));

        command = String::from("-Q -e fuzz -v");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, Some("fuzz".to_string()));

        command = String::from("-Q --extension fuzz -v --extension ");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, None);

        command = String::from("-Q --extension fuzz -v -e ");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, None);

        let mut command = String::from("--extension buzz -e fuzz");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, Some("fuzz".to_string()));

        command = String::from("-Q -v -e fuzz");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, Some("fuzz".to_string()));

        command = String::from("-Q -v -efuzz");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, Some("fuzz".to_string()));

        command = String::from("-Q -v --ext fuzz");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, Some("fuzz".to_string()));

        command = String::from("-Q -v --extfuzz");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, None);

        command = String::from("-Q -v --workspace");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "--ext");
        assert_eq!(extension, None);

        command = String::from("-Q -v -e");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "", "--ext");
        assert_eq!(extension, None);

        command = String::from("-Q -v --ext fuzz");
        let args = command.split_whitespace();

        let extension = get_cmd_option_value(args, "-e", "");
        assert_eq!(extension, None);
    }

    #[test]
    fn test_get_crash_dir_and_ext() {
        pub const TARGET: &str = "fuzz_0";
        pub const TEST_CRASH_PATH: &str = "/home/fuzz/test-crash-path";

        const ROOT: &str = "/home/fuzz/";

        let default_crash_path = std::path::Path::new(HFUZZ_WORKSPACE_DEFAULT).join(TARGET);
        let env_specified_crash_path = std::path::Path::new(TEST_CRASH_PATH).join(TARGET);

        // this is default behavior
        let (crash_dir, ext) = get_crash_dir_and_ext(ROOT, TARGET, "", HFUZZ_WORKSPACE_DEFAULT);

        assert_eq!(crash_dir, default_crash_path);
        assert_eq!(&ext, "fuzz");

        // behavior where path is specified within env variable HFUZZ_WORKSPACE, but not within -W HFUZZ_RUN_ARGS
        let (crash_dir, ext) = get_crash_dir_and_ext(ROOT, TARGET, "-Q -e", TEST_CRASH_PATH);

        assert_eq!(crash_dir, env_specified_crash_path);
        assert_eq!(&ext, "fuzz");

        // behavior as above
        let (crash_dir, ext) = get_crash_dir_and_ext(ROOT, TARGET, "-Q -e crash", TEST_CRASH_PATH);

        assert_eq!(crash_dir, env_specified_crash_path);
        assert_eq!(&ext, "crash");

        // test absolute path
        // HFUZZ_WORKSPACE has default value however -W is set
        let (crash_dir, ext) = get_crash_dir_and_ext(
            ROOT,
            TARGET,
            "-Q -W /home/crash -e crash",
            HFUZZ_WORKSPACE_DEFAULT,
        );

        let expected_crash_path = std::path::Path::new("/home/crash");
        assert_eq!(crash_dir, expected_crash_path);
        assert_eq!(&ext, "crash");

        // test absolute path
        // HFUZZ_WORKSPACE is set and -W is also set
        let (crash_dir, ext) = get_crash_dir_and_ext(
            ROOT,
            TARGET,
            "-Q --crash /home/crash -e crash",
            TEST_CRASH_PATH,
        );
        let expected_crash_path = std::path::Path::new("/home/crash");
        assert_eq!(crash_dir, expected_crash_path);
        assert_eq!(&ext, "crash");

        // test absolute path
        // HFUZZ_WORKSPACE is set and -W is also set
        let (crash_dir, ext) = get_crash_dir_and_ext(
            ROOT,
            TARGET,
            "-Q --crash /home/crash/foo/bar/dead/beef -e crash",
            TEST_CRASH_PATH,
        );

        let expected_crash_path = std::path::Path::new("/home/crash/foo/bar/dead/beef");
        assert_eq!(crash_dir, expected_crash_path);
        assert_eq!(&ext, "crash");

        // test relative path
        // HFUZZ_WORKSPACE is set and -W is also set, this time with relative path
        let (crash_dir, ext) =
            get_crash_dir_and_ext(ROOT, TARGET, "-Q -W ../crash -e crash", TEST_CRASH_PATH);

        let expected_crash_path = std::path::Path::new(ROOT).join("../crash");
        assert_eq!(crash_dir, expected_crash_path);
        assert_eq!(&ext, "crash");

        // test relative path
        // HFUZZ_WORKSPACE is set and -W is also set, this time with relative path
        let (crash_dir, ext) = get_crash_dir_and_ext(
            ROOT,
            TARGET,
            "-Q -W ../../dead/beef/crash -e crash",
            TEST_CRASH_PATH,
        );

        let expected_crash_path = std::path::Path::new(ROOT).join("../../dead/beef/crash");
        assert_eq!(crash_dir, expected_crash_path);
        assert_eq!(&ext, "crash");

        // test relative path
        let (crash_dir, ext) = get_crash_dir_and_ext(
            ROOT,
            TARGET,
            "-Q --crash ../crash -e crash",
            HFUZZ_WORKSPACE_DEFAULT,
        );

        let expected_crash_path = std::path::Path::new(ROOT).join("../crash");
        assert_eq!(crash_dir, expected_crash_path);
        assert_eq!(&ext, "crash");

        // crash directory has precedence before workspace option , which have precedence before
        // HFUZZ_WORKSPACE
        let (crash_dir, ext) = get_crash_dir_and_ext(
            ROOT,
            TARGET,
            "-Q --crash ../bitcoin/to/the/moon -W /workspace -e crash",
            TEST_CRASH_PATH,
        );

        let expected_crash_path = std::path::Path::new(ROOT).join("../bitcoin/to/the/moon");
        assert_eq!(crash_dir, expected_crash_path);
        assert_eq!(&ext, "crash");

        // crash directory has precedence before workspace HFUZZ_WORKSPACE
        let (crash_dir, ext) = get_crash_dir_and_ext(
            ROOT,
            TARGET,
            "-Q --crash /home/crashes/we/like/solana -e crash",
            TEST_CRASH_PATH,
        );

        // If path is specified as absolute, the join will replace whole path.
        let expected_crash_path = std::path::Path::new("/home/crashes/we/like/solana");

        // let expected_crash_path = root.join("/home/crashes/we/like/solana");
        assert_eq!(crash_dir, expected_crash_path);
        assert_eq!(&ext, "crash");
    }
}
