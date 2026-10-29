//! A compilation database for a C, C++ or CUDA project that has none, so that
//! clangd reads each file as what it is and finds the project's headers.
//!
//! Without a database clangd compiles each file on its own, with no include
//! path: a project's own headers would not be found, a `.cu` file would fail
//! for want of a CUDA installation, and a `.cuh` header — an extension clang
//! does not know — would make no compile job at all, so every request on it
//! would fail with `-32001 invalid AST`. The database is written under
//! Devscriptor's cache, never into the project.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

/// The placeholder in a server's registry arguments for the directory of the
/// compilation database it reads. An argument holding it is left out where
/// the project configures clangd itself (see [`database_dir`]).
pub const COMPILE_COMMANDS_DIR_PLACEHOLDER: &str = "{compile_commands_dir}";

/// How many files a database lists at most: past that, a tree is mostly
/// vendored or generated code, and clangd infers the commands of the files
/// left out from those of the files nearest them.
const MAX_FILES: usize = 20_000;

/// The directories a walk does not enter: version control, dependencies,
/// virtual environments and build output.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".cache",
    ".venv",
    "venv",
    "node_modules",
    "target",
    "build",
    "out",
    "dist",
    "__pycache__",
];

/// The names of the directories a project keeps headers in, which every file
/// is given on its include path.
const INCLUDE_DIRS: &[&str] = &["include", "inc"];

/// The flags that let clang parse CUDA where no CUDA installation is: for
/// the host side only, without the installation's headers and libraries, and
/// with the attributes those headers would define (`__device__`,
/// `__global__`, …) and the built-in variables (`threadIdx`) clang ships.
const CUDA_WITHOUT_INSTALLATION: &[&str] = &[
    "-xcuda",
    "--cuda-host-only",
    "-nocudainc",
    "-nocudalib",
    "-D__host__=__attribute__((host))",
    "-D__device__=__attribute__((device))",
    "-D__global__=__attribute__((global))",
    "-D__shared__=__attribute__((shared))",
    "-D__constant__=__attribute__((constant))",
    "-D__managed__=__attribute__((managed))",
    "-include",
    "__clang_cuda_builtin_vars.h",
];

/// The directory clangd reads a compilation database from for a server over
/// `folders`: `None` when one of them configures clangd itself — a
/// `compile_commands.json` at its root or in `build/`, a `compile_flags.txt`
/// or a `.clangd`, which clangd finds on its own, and which a database passed
/// to it would override — or when they hold no C, C++ or CUDA file; else one
/// written for them under `cache_dir`, where the databases of workspaces
/// since deleted are then removed ([`forget_stale_databases`]).
#[must_use]
pub fn database_dir(folders: &[PathBuf], cache_dir: &Path) -> Option<PathBuf> {
    let configured = folders.iter().any(|folder| {
        [
            "compile_commands.json",
            "build/compile_commands.json",
            "compile_flags.txt",
            ".clangd",
        ]
        .iter()
        .any(|config| folder.join(config).exists())
    });
    if configured {
        return None;
    }
    let cuda = cuda_installation();
    let commands: Vec<serde_json::Value> = folders
        .iter()
        .flat_map(|folder| commands_for(folder, cuda.as_deref()))
        .collect();
    if commands.is_empty() {
        return None;
    }
    let databases = cache_dir.join("clangd");
    let dir = databases.join(folders_key(folders));
    let written = std::fs::create_dir_all(&dir).and_then(|()| {
        let staged = dir.join("compile_commands.json.tmp");
        std::fs::write(&staged, serde_json::to_vec_pretty(&commands)?)?;
        std::fs::rename(&staged, dir.join("compile_commands.json"))
    });
    match written {
        Ok(()) => {
            forget_stale_databases(&databases);
            Some(dir)
        }
        Err(e) => {
            tracing::warn!(
                "No compilation database for clangd in {}: {e}",
                dir.display()
            );
            None
        }
    }
}

/// The compile command of each C, C++ and CUDA file under `root`: its
/// language (a `.h` C++'s where the project holds C++ or CUDA, else C's), the
/// project's include directories, and, for CUDA, the installation at `cuda`
/// or the flags that do without one.
///
/// A directory below `root` that configures clangd keeps its files as it
/// says: its own compilation database (`compile_commands.json`, or one in
/// `build/` as CMake writes it) gives their commands as they are, and its
/// `compile_flags.txt` their flags, read from that directory, in place of the
/// include directories. Passed a database, clangd reads no other, so a
/// project's own, inside the folder, would be lost to the commands written here.
fn commands_for(root: &Path, cuda: Option<&Path>) -> Vec<serde_json::Value> {
    let mut files = Vec::new();
    let mut includes = vec![root.to_path_buf()];
    let mut databases: Vec<(PathBuf, Vec<serde_json::Value>)> = Vec::new();
    let mut flagged: Vec<(PathBuf, Vec<String>)> = Vec::new();
    let entries = walkdir::WalkDir::new(root)
        .follow_links(false)
        .max_depth(16)
        .into_iter()
        .filter_entry(|entry| {
            !(entry.file_type().is_dir()
                && entry.depth() > 0
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| SKIPPED_DIRS.contains(&name)))
        })
        .filter_map(Result::ok);
    for entry in entries {
        if entry.file_type().is_dir() {
            let dir = entry.path();
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| INCLUDE_DIRS.contains(&name))
            {
                includes.push(dir.to_path_buf());
            }
            if entry.depth() > 0 {
                if let Some(commands) = own_database(dir) {
                    databases.push((dir.to_path_buf(), commands));
                } else if let Some(flags) = own_flags(dir) {
                    flagged.push((dir.to_path_buf(), flags));
                }
            }
        } else if language_of(entry.path(), true).is_some() && files.len() < MAX_FILES {
            files.push(entry.path().to_path_buf());
        }
    }
    let listed: HashSet<PathBuf> = databases
        .iter()
        .flat_map(|(_, commands)| commands)
        .filter_map(|command| command["file"].as_str().map(PathBuf::from))
        .collect();
    let cpp = files
        .iter()
        .any(|file| matches!(language_of(file, true), Some("c++" | "c++-header" | "cuda")));
    let include_flags: Vec<String> = includes
        .iter()
        .map(|dir| format!("-I{}", dir.display()))
        .collect();
    let written = files
        .iter()
        .filter(|file| {
            !listed.contains(*file) && !databases.iter().any(|(dir, _)| file.starts_with(dir))
        })
        .filter_map(|file| {
            let language = language_of(file, cpp)?;
            let mut arguments = vec!["clang".to_string()];
            if language == "cuda" {
                match cuda {
                    Some(installation) => arguments.extend([
                        "-xcuda".to_string(),
                        "--cuda-host-only".to_string(),
                        format!("--cuda-path={}", installation.display()),
                    ]),
                    None => {
                        arguments.extend(CUDA_WITHOUT_INSTALLATION.iter().map(|f| f.to_string()))
                    }
                }
            } else {
                arguments.push(format!("-x{language}"));
            }
            // The innermost `compile_flags.txt` above the file gives its flags,
            // read from its directory as clangd reads them.
            let own = flagged
                .iter()
                .filter(|(dir, _)| file.starts_with(dir))
                .max_by_key(|(dir, _)| dir.components().count());
            let directory = match own {
                Some((dir, flags)) => {
                    arguments.extend(flags.iter().cloned());
                    dir.as_path()
                }
                None => {
                    arguments.extend(include_flags.iter().cloned());
                    root
                }
            };
            arguments.extend(["-c".to_string(), file.display().to_string()]);
            Some(serde_json::json!({
                "directory": directory.display().to_string(),
                "file": file.display().to_string(),
                "arguments": arguments,
            }))
        })
        .collect::<Vec<_>>();
    written
        .into_iter()
        .chain(databases.into_iter().flat_map(|(_, commands)| commands))
        .collect()
}

/// The commands of the compilation database `dir` keeps for itself, in
/// `compile_commands.json` there or in its `build/`, each file's path made
/// absolute against its `directory`; `None` without one that parses.
fn own_database(dir: &Path) -> Option<Vec<serde_json::Value>> {
    ["compile_commands.json", "build/compile_commands.json"]
        .iter()
        .find_map(|name| {
            let text = std::fs::read(dir.join(name)).ok()?;
            serde_json::from_slice::<Vec<serde_json::Value>>(&text).ok()
        })
        .map(|commands| {
            commands
                .into_iter()
                .map(|mut command| {
                    if let (Some(directory), Some(file)) =
                        (command["directory"].as_str(), command["file"].as_str())
                    {
                        let file = lexical(&Path::new(directory).join(file));
                        command["file"] = serde_json::json!(file.display().to_string());
                    }
                    command
                })
                .collect()
        })
}

/// The flags `dir`'s `compile_flags.txt` gives the files under it, one a
/// line; `None` without one.
fn own_flags(dir: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(dir.join("compile_flags.txt")).ok()?;
    Some(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// `path` with its `.` and `..` resolved by its own components, as a
/// compilation database's `../src/a.cpp` from `build/` names `src/a.cpp`.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The language clang compiles `file` as, by its extension — a `.h` as C++'s
/// header when `cpp`, else C's, and a `.m` as Objective-C when it is not
/// MATLAB's — or `None` for a file clangd does not serve.
fn language_of(file: &Path, cpp: bool) -> Option<&'static str> {
    match file.extension()?.to_str()? {
        "c" => Some("c"),
        "h" => Some(if cpp { "c++-header" } else { "c-header" }),
        "cc" | "cpp" | "cxx" | "c++" => Some("c++"),
        "hh" | "hpp" | "hxx" | "h++" | "ipp" | "tpp" => Some("c++-header"),
        "cu" | "cuh" => Some("cuda"),
        "mm" => Some("objective-c++"),
        "m" => crate::languages::is_objective_c(file).then_some("objective-c"),
        _ => None,
    }
}

/// Where a CUDA installation is: `CUDA_PATH`, `CUDA_HOME` or `CUDA_ROOT`, or
/// `/usr/local/cuda`, whichever holds `include/cuda_runtime.h`.
fn cuda_installation() -> Option<PathBuf> {
    ["CUDA_PATH", "CUDA_HOME", "CUDA_ROOT"]
        .iter()
        .filter_map(|var| std::env::var_os(var).map(PathBuf::from))
        .chain(std::iter::once(PathBuf::from("/usr/local/cuda")))
        .find(|dir| dir.join("include").join("cuda_runtime.h").exists())
}

/// Remove each database under `databases` that names files of which none is
/// there any more: a deleted workspace's — a test's temporary directory, a
/// removed worktree — which no server will read again, and which would
/// otherwise stay, one for every set of folders a server was started over.
/// One that cannot be read is left alone, as it may be being written.
fn forget_stale_databases(databases: &Path) {
    let Ok(entries) = std::fs::read_dir(databases) else {
        return;
    };
    for dir in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        if names_only_missing_files(&dir.join("compile_commands.json"))
            && let Err(e) = std::fs::remove_dir_all(&dir)
        {
            tracing::debug!("Could not remove the stale database {}: {e}", dir.display());
        }
    }
}

/// Whether the compilation database at `path` names files, and none of them
/// exists: each a command's `file`, taken from its `directory` when relative.
fn names_only_missing_files(path: &Path) -> bool {
    let Some(commands) = std::fs::read(path)
        .ok()
        .and_then(|text| serde_json::from_slice::<Vec<serde_json::Value>>(&text).ok())
    else {
        return false;
    };
    let files: Vec<PathBuf> = commands
        .iter()
        .filter_map(|command| {
            let file = Path::new(command.get("file")?.as_str()?);
            Some(match command.get("directory").and_then(|d| d.as_str()) {
                Some(directory) if file.is_relative() => Path::new(directory).join(file),
                _ => file.to_path_buf(),
            })
        })
        .collect();
    !files.is_empty() && files.iter().all(|file| !file.exists())
}

/// The name of the database directory of a server over `folders`: their
/// paths' digest, so that one project's database is found again and another
/// project's is never it.
fn folders_key(folders: &[PathBuf]) -> String {
    let mut hasher = Sha256::new();
    for folder in folders {
        hasher.update(folder.to_string_lossy().as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (file, text) in [
            ("include/kernels.cuh", "__device__ float scale(float x);\n"),
            ("include/util.h", "int twice(int x);\n"),
            ("src/run.cu", "#include \"kernels.cuh\"\n"),
            ("src/main.cpp", "#include \"util.h\"\n"),
            ("build/generated.cpp", "int generated;\n"),
            (
                "src/Legacy.m",
                "#import \"util.h\"\n@implementation Legacy\n@end\n",
            ),
            ("tools/plot.m", "function plot_limits(x)\n  disp(x);\nend\n"),
            ("README.md", "# A project\n"),
        ] {
            let path = root.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        root
    }

    /// Each file is compiled as its language — a `.cuh` header as CUDA, a
    /// `.h` as C++'s where the project holds C++, an Objective-C `.m` as
    /// Objective-C — with the project's include directories; build output is
    /// not listed, nor what clangd does not serve, a MATLAB `.m` among it.
    #[test]
    fn each_file_is_compiled_as_what_it_is() {
        let root = project();
        let commands = commands_for(root.path(), None);
        let by_file = |name: &str| -> Vec<String> {
            commands
                .iter()
                .find(|command| command["file"].as_str().unwrap().ends_with(name))
                .unwrap_or_else(|| panic!("no command for {name}: {commands:?}"))["arguments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|argument| argument.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(commands.len(), 5, "{commands:?}");
        assert_eq!(by_file("Legacy.m")[1], "-xobjective-c");
        assert!(
            !commands
                .iter()
                .any(|command| command["file"].as_str().unwrap().ends_with("plot.m")),
            "{commands:?}"
        );
        for cuda in ["kernels.cuh", "run.cu"] {
            let arguments = by_file(cuda);
            assert_eq!(
                arguments[1..5],
                ["-xcuda", "--cuda-host-only", "-nocudainc", "-nocudalib"]
            );
            assert!(arguments.contains(&"__clang_cuda_builtin_vars.h".to_string()));
        }
        assert_eq!(by_file("util.h")[1], "-xc++-header");
        assert_eq!(by_file("main.cpp")[1], "-xc++");
        let include = format!("-I{}", root.path().join("include").display());
        assert!(by_file("main.cpp").contains(&include));
        let installed = commands_for(root.path(), Some(Path::new("/opt/cuda")));
        let run = installed
            .iter()
            .find(|command| command["file"].as_str().unwrap().ends_with("run.cu"))
            .unwrap();
        assert_eq!(run["arguments"][3], "--cuda-path=/opt/cuda");
    }

    /// A project inside the folder that configures clangd keeps its files as
    /// it says: its own compilation database's commands, as written (their
    /// files' paths made absolute), with none written for a file of it the
    /// database leaves out, such as a header, whose command clangd infers from
    /// the project's; and its `compile_flags.txt`, read from its directory.
    /// Passed the database written here, clangd read no other.
    #[test]
    fn a_project_inside_keeps_its_own_commands() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let svc = root.join("services/svc");
        for (file, text) in [
            ("services/svc/src/a.cpp", "int a;\n".to_string()),
            ("services/svc/include/a.hpp", "int a();\n".to_string()),
            (
                "services/svc/build/compile_commands.json",
                serde_json::json!([{
                    "directory": svc.join("build").display().to_string(),
                    "file": "../src/a.cpp",
                    "arguments": ["clang++", "-DSVC=1", "-c", "../src/a.cpp"],
                }])
                .to_string(),
            ),
            ("lib/compile_flags.txt", "-Ivendor\n-DLIB\n".to_string()),
            ("lib/x.c", "int x;\n".to_string()),
            ("tools/b.c", "int b;\n".to_string()),
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let commands = commands_for(root, None);
        let of = |name: &str| -> Vec<&serde_json::Value> {
            commands
                .iter()
                .filter(|command| command["file"].as_str().unwrap().ends_with(name))
                .collect()
        };
        let has = |command: &serde_json::Value, argument: &str| {
            command["arguments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a == argument)
        };
        let root_include = format!("-I{}", root.display());

        let a = of("a.cpp");
        assert_eq!(a.len(), 1, "{commands:?}");
        assert!(has(a[0], "-DSVC=1"), "{:?}", a[0]);
        assert_eq!(
            Path::new(a[0]["file"].as_str().unwrap()),
            svc.join("src/a.cpp")
        );
        assert!(of("a.hpp").is_empty(), "{commands:?}");

        let x = of("x.c");
        assert!(has(x[0], "-Ivendor") && has(x[0], "-DLIB"), "{:?}", x[0]);
        assert!(!has(x[0], &root_include), "{:?}", x[0]);
        assert_eq!(x[0]["directory"], root.join("lib").display().to_string());

        let b = of("b.c");
        assert!(has(b[0], &root_include), "{:?}", b[0]);
        assert_eq!(b[0]["directory"], root.display().to_string());
    }

    /// A project that configures clangd itself is left to it; another gets a
    /// database under the cache, found again for the same folders.
    #[test]
    fn a_database_is_written_only_where_the_project_has_none() {
        let root = project();
        let cache = tempfile::tempdir().unwrap();
        let folders = vec![root.path().to_path_buf()];
        let dir = database_dir(&folders, cache.path()).expect("a database");
        assert!(dir.starts_with(cache.path()));
        let written: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(dir.join("compile_commands.json")).unwrap())
                .unwrap();
        assert_eq!(written.len(), 5);
        assert_eq!(database_dir(&folders, cache.path()), Some(dir));

        std::fs::write(root.path().join("compile_flags.txt"), "-Iinclude\n").unwrap();
        assert_eq!(database_dir(&folders, cache.path()), None);

        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            database_dir(&[empty.path().to_path_buf()], cache.path()),
            None
        );
    }

    /// Writing a database removes those whose files are all gone, a deleted
    /// workspace's; one with a file left stays, as do one that names its
    /// files relative to their directory, one that names none, and one that
    /// cannot be read.
    #[test]
    fn the_databases_of_deleted_workspaces_go() {
        let cache = tempfile::tempdir().unwrap();
        let deleted = project();
        let stale = database_dir(&[deleted.path().to_path_buf()], cache.path()).unwrap();
        let changed = project();
        let partly = database_dir(&[changed.path().to_path_buf()], cache.path()).unwrap();
        std::fs::remove_file(changed.path().join("src/main.cpp")).unwrap();
        let other = |name: &str, database: String| {
            let dir = cache.path().join("clangd").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("compile_commands.json"), database).unwrap();
            dir
        };
        let relative = other(
            "relative",
            serde_json::json!([{
                "directory": changed.path().join("src").display().to_string(),
                "file": "run.cu",
                "arguments": ["clang++", "run.cu"]
            }])
            .to_string(),
        );
        let empty = other("empty", "[]".into());
        let unreadable = other("unreadable", "[{".into());
        drop(deleted);
        assert!(stale.exists(), "nothing goes until a database is written");

        let current = project();
        let written = database_dir(&[current.path().to_path_buf()], cache.path()).unwrap();
        assert!(!stale.exists(), "the deleted workspace's database stays");
        assert!(partly.exists(), "a database with a file left goes");
        assert!(
            relative.exists(),
            "a file named from its directory is not found"
        );
        assert!(empty.exists() && unreadable.exists());
        assert!(written.join("compile_commands.json").exists());
    }
}
