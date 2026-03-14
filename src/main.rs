use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

const CONFIG_FILE: &str = "config.txt";

struct Mapping {
    local: PathBuf,
    phone: String,
}

fn config_error(msg: &str) -> ! {
    eprintln!("config error: {msg}");
    eprintln!("expected a '{CONFIG_FILE}' file with lines like:");
    eprintln!("  /home/user/music -> /sdcard/Music");
    std::process::exit(1);
}

fn parse_config() -> Vec<Mapping> {
    let file = match fs::File::open(CONFIG_FILE) {
        Ok(file) => file,
        Err(error) => config_error(&format!("cannot open '{CONFIG_FILE}': {error}")),
    };
    BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(line_index, line_result)| {
            let line = match line_result {
                Ok(line_content) => line_content,
                Err(error) => config_error(&format!("failed to read line {}: {error}", line_index + 1)),
            };
            let line = line.trim().to_string();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (local, phone) = match line.split_once("->") {
                Some(pair) => pair,
                None => config_error(&format!(
                    "line {} missing '->': {line}", line_index + 1
                )),
            };
            Some(Mapping {
                local: PathBuf::from(local.trim()),
                phone: phone.trim().to_string(),
            })
        })
        .collect()
}

enum AdbError {
    NotInstalled(std::io::Error),
    NoDevice,
}

impl std::fmt::Display for AdbError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled(error) => write!(formatter, "cannot run adb: {error} — is it installed?"),
            Self::NoDevice => write!(formatter, "no android device connected (check 'adb devices')"),
        }
    }
}

fn adb(args: &[&str]) -> Result<String, String> {
    let output = Command::new("adb")
        .args(args)
        .output()
        .map_err(|error| format!("failed to run adb: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn adb_check() -> Result<(), AdbError> {
    let output = Command::new("adb")
        .args(["devices"])
        .output()
        .map_err(AdbError::NotInstalled)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let has_device = stdout
        .lines()
        .skip(1)
        .any(|line| line.contains("device"));
    if has_device { Ok(()) } else { Err(AdbError::NoDevice) }
}

/// Returns map of relative_path -> mtime (epoch seconds) for all files under
/// the given phone directory.
fn phone_files(phone_dir: &str) -> HashMap<String, u64> {
    let shell_command = format!(
        "if [ -d '{phone_dir}' ]; then find '{phone_dir}' -type f -exec stat -c '%Y %n' {{}} +; fi"
    );
    let output = match adb(&["shell", &shell_command]) {
        Ok(stdout) => stdout,
        Err(_) => return HashMap::new(),
    };
    let prefix_len = phone_dir.trim_end_matches('/').len() + 1; // +1 for the /
    output.lines()
        .filter_map(|line| {
            let (timestamp, file_path) = line.split_once(' ')?;
            let modified_time: u64 = timestamp.parse().ok()?;
            if file_path.len() <= prefix_len {
                return None;
            }
            let relative_path = &file_path[prefix_len..];
            Some((relative_path.to_string(), modified_time))
        })
        .collect()
}

fn local_files(directory: &Path) -> Vec<(PathBuf, u64)> {
    let mut collected_files = Vec::new();
    walk(directory, directory, &mut collected_files);
    collected_files
}

fn walk(base_dir: &Path, current_dir: &Path, collected_files: &mut Vec<(PathBuf, u64)>) {
    let entries = match fs::read_dir(current_dir) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("warning: cannot read {}: {error}", current_dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            walk(base_dir, &entry_path, collected_files);
        } else if entry_path.is_file() {
            let modified_time = entry_path
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map(|system_time| system_time.duration_since(UNIX_EPOCH).unwrap().as_secs())
                .unwrap_or(0);
            let relative_path = entry_path.strip_prefix(base_dir).unwrap().to_path_buf();
            collected_files.push((relative_path, modified_time));
        }
    }
}

/// synced folders in the destination will be exactly like the source
fn sync(mapping: &Mapping, dry_run: bool) {
    println!(
        "\n=== {} -> {} ===",
        mapping.local.display(),
        mapping.phone
    );
    if !mapping.local.is_dir() {
        eprintln!(
            "warning: local directory '{}' does not exist, skipping",
            mapping.local.display()
        );
        return;
    }

    let remote_files = phone_files(&mapping.phone);
    let local_file_list = local_files(&mapping.local);
    let mut push_count = 0u32;
    let local_set: std::collections::HashSet<String> = local_file_list
        .iter()
        .map(|(path, _)| path.to_string_lossy().into_owned())
        .collect();

    for (relative_path, local_modified_time) in &local_file_list {
        let relative_path_str = relative_path.to_string_lossy();
        let remote_file = remote_files.get(relative_path_str.as_ref());
        let needs_push = match remote_file {
            Some(&phone_modified_time) => *local_modified_time > phone_modified_time,
            None => true,
        };
        if !needs_push {
            continue;
        }

        let local_file_path = mapping.local.join(relative_path);
        let remote_file_path = format!("{}/{relative_path_str}", mapping.phone);

        if dry_run {
            println!("  [dry-run] would push {relative_path_str}");
        } else {
            // ensure parent dir exists on phone
            if let Some(parent_dir) = Path::new(&remote_file_path).parent() {
                let _ = adb(&["shell", "mkdir", "-p", &parent_dir.to_string_lossy()]);
            }
            print!("  pushing {relative_path_str} ... ");
            match adb(&["push", &local_file_path.to_string_lossy(), &remote_file_path]) {
                Ok(_) => println!("ok"),
                Err(error) => println!("FAILED: {error}"),
            }
        }
        push_count += 1;
    }

    // delete files on phone that don't exist locally
    let mut delete_count = 0u32;
    for remote_relative in remote_files.keys() {
        if local_set.contains(remote_relative) {
            continue;
        }
        let remote_file_path = format!("{}/{remote_relative}", mapping.phone);
        if dry_run {
            println!("  [dry-run] would delete {remote_relative}");
        } else {
            print!("  deleting {remote_relative} ... ");
            match adb(&["shell", "rm", &remote_file_path]) {
                Ok(_) => println!("ok"),
                Err(error) => println!("FAILED: {error}"),
            }
        }
        delete_count += 1;
    }

    // remove empty directories left behind
    if delete_count > 0 && !dry_run {
        let _ = adb(&[
            "shell",
            "find",
            &mapping.phone,
            "-type",
            "d",
            "-empty",
            "-delete",
        ]);
    }

    if push_count == 0 && delete_count == 0 {
        println!("  everything up to date");
    } else {
        let verb = if dry_run { "to push" } else { "pushed" };
        let del_verb = if dry_run { "to delete" } else { "deleted" };
        if push_count > 0 {
            println!("  {push_count} file(s) {verb}");
        }
        if delete_count > 0 {
            println!("  {delete_count} file(s) {del_verb}");
        }
    }
}

fn main() {
    let dry_run = std::env::args().any(|arg| arg == "--dry-run");
    let mappings = parse_config();
    if mappings.is_empty() {
        eprintln!("no mappings in '{CONFIG_FILE}'");
        std::process::exit(1);
    }
    if let Err(error) = adb_check() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
    for mapping in &mappings {
        sync(mapping, dry_run);
    }
}
