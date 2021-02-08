use fs::File;
use io::{BufRead, Read, Write};
use path::{Path, PathBuf};
use std::{env, fmt, fs, io, path};

enum Error {
    GitDirNotFound,
    Io(io::Error),
    OutDir(env::VarError),
    InvalidUserHooksDir(PathBuf),
    EmptyUserHook(PathBuf),
}

type Result<T> = std::result::Result<T, Error>;

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

impl From<env::VarError> for Error {
    fn from(error: env::VarError) -> Error {
        Error::OutDir(error)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let msg = match self {
            Error::GitDirNotFound => format!(
                ".git directory was not found in '{}' or its parent directories",
                env::var("OUT_DIR").unwrap_or_else(|_| "".to_string()),
            ),
            Error::Io(inner) => format!("IO error: {}", inner),
            Error::OutDir(env::VarError::NotPresent) => unreachable!(),
            Error::OutDir(env::VarError::NotUnicode(msg)) => msg.to_string_lossy().to_string(),
            Error::InvalidUserHooksDir(path) => {
                format!("User hooks directory is not found or no executable file is found in '{:?}'. Did you forget to make a hook script executable?", path)
            }
            Error::EmptyUserHook(path) => format!("User hook script is empty: {:?}", path),
        };
        write!(f, "{}", msg)
    }
}

fn resolve_gitdir() -> Result<PathBuf> {
    let dir = env::var("OUT_DIR")?;
    let mut dir = PathBuf::from(dir);
    if !dir.has_root() {
        dir = fs::canonicalize(dir)?;
    }
    loop {
        let gitdir = dir.join(".git");
        if gitdir.is_dir() {
            return Ok(gitdir);
        }
        if gitdir.is_file() {
            let mut buf = String::new();
            File::open(gitdir)?.read_to_string(&mut buf)?;
            let newlines: &[_] = &['\n', '\r'];
            let gitdir = PathBuf::from(buf.trim_end_matches(newlines));
            if !gitdir.is_dir() {
                return Err(Error::GitDirNotFound);
            }
            return Ok(gitdir);
        }
        if !dir.pop() {
            return Err(Error::GitDirNotFound);
        }
    }
}

// This function returns true when
//   - the hook was generated by the same version of cargo-husky
//   - someone else had already put another hook script
// For safety, cargo-husky does nothing on case2 also.
fn hook_already_exists(hook: &Path) -> bool {
    let f = match File::open(hook) {
        Ok(f) => f,
        Err(..) => return false,
    };

    let ver_line = match io::BufReader::new(f).lines().nth(2) {
        None => return true, // Less than 2 lines. The hook script seemed to be generated by someone else
        Some(Err(..)) => return false, // Failed to read entry. Re-generate anyway
        Some(Ok(line)) => line,
    };

    ver_line.contains(&format!(
        "# This hook was set for {} v{}: {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_HOMEPAGE")
    ))
}

fn create_executable_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(path)
}

fn install_hook(src: &Path, dst: &Path) -> Result<()> {
    if hook_already_exists(dst) {
        return Ok(());
    }

    let mut lines = {
        let mut vec = vec![];
        for line in io::BufReader::new(File::open(src)?).lines() {
            vec.push(line?);
        }
        vec
    };

    if lines.is_empty() {
        return Err(Error::EmptyUserHook(src.to_owned()));
    }

    // Insert project package version information as comment
    if !lines[0].starts_with("#!") {
        lines.insert(0, "#".to_string());
    }
    lines.insert(1, "#".to_string());
    lines.insert(
        2,
        format!(
            "# This hook was set for {} v{}: {}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_HOMEPAGE")
        ),
    );

    let dst_file_path = dst.join(src.file_name().unwrap());

    let mut f = io::BufWriter::new(create_executable_file(&dst_file_path)?);
    for line in lines {
        writeln!(f, "{}", line)?;
    }

    Ok(())
}

fn is_executable_file(entry: &fs::DirEntry) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let ft = match entry.file_type() {
        Ok(ft) => ft,
        Err(..) => return false,
    };
    if !ft.is_file() {
        return false;
    }
    let md = match entry.metadata() {
        Ok(md) => md,
        Err(..) => return false,
    };
    let mode = md.permissions().mode();
    mode & 0o555 == 0o555 // Check file is read and executable mode
}

fn install_hooks() -> Result<()> {
    let git_dir = resolve_gitdir()?;
    let user_hooks_dir = {
        let mut p = git_dir.clone();
        p.pop();
        p.push(".project");
        p.push("hooks");
        p
    };

    if !user_hooks_dir.is_dir() {
        return Err(Error::InvalidUserHooksDir(user_hooks_dir));
    }

    let hook_paths = fs::read_dir(&user_hooks_dir)?
        .filter_map(|e| e.ok().filter(is_executable_file).map(|e| e.path()))
        .collect::<Vec<_>>();

    if hook_paths.is_empty() {
        return Err(Error::InvalidUserHooksDir(user_hooks_dir));
    }

    let hooks_dir = git_dir.join("hooks");
    if !hooks_dir.exists() {
        fs::create_dir(hooks_dir.as_path())?;
    }
    for path in hook_paths {
        install_hook(&path, &hooks_dir)?;
    }

    Ok(())
}

fn install() -> Result<()> {
    install_hooks()
}

fn main() -> Result<()> {
    match install() {
        Err(e @ Error::GitDirNotFound) => {
            // #2
            eprintln!("Warning: {:?}", e);
            Ok(())
        }
        otherwise => otherwise,
    }
}
