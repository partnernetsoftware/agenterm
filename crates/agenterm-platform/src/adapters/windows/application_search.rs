//! Windows executable resolution shared by every native spawn path.
//!
//! `CreateProcessW` only searches `PATH` when `lpApplicationName` is null.
//! Both the ConPTY spawn and the contained-process spawn pass the program as
//! `lpApplicationName` so that a mis-resolved image names itself in `GetLastError`
//! diagnostics, so each must resolve a bare name first. Keeping one resolver
//! here means a bare `rustc`, `cargo`, or `git` resolves identically however it
//! was spawned; duplicating the search in two adapters is how one of them ends
//! up missing `.COM`, `PATHEXT`, or an environment override.
//!
//! The search mirrors the documented `CreateProcessW` order for a name with no
//! path component: each `PATH` directory, each `PATHEXT` extension (restricted
//! to `.exe` / `.com`, the only images `CreateProcessW` can start directly).

use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::OsStrExt as _,
    path::{Path, PathBuf},
};

/// Resolve `program` to an absolute image path before `CreateProcessW`.
///
/// `program` is used verbatim when it already names a path (absolute or with a
/// path component). A bare name is searched against `current_dir` — falling
/// back to the process working directory — and then every `PATH` entry.
/// `env` is the spawn-time override list (last entry wins, `None` removes), so
/// a caller that sets or clears `PATH` while spawning gets that exact value,
/// not the parent's.
///
/// Returns [`io::ErrorKind::NotFound`] when nothing matches, naming `context`
/// so the caller's diagnostics stay specific.
pub(crate) fn resolve_application_path(
    program: &Path,
    context: &str,
    current_dir: Option<&Path>,
    env: &[(OsString, Option<OsString>)],
) -> io::Result<PathBuf> {
    if program.is_absolute() || has_path_component(program) {
        let base = match current_dir {
            Some(current_dir) => current_dir.to_owned(),
            None => env::current_dir()?,
        };
        let candidate = if program.is_absolute() {
            program.to_owned()
        } else {
            base.join(program)
        };
        let pathext = effective_env_value(env, "PATHEXT");
        if let Some(path) = resolve_application_candidate(&candidate, pathext.as_deref())? {
            return Ok(path);
        }
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{context} not found: {}", candidate.to_string_lossy()),
        ));
    }

    let Some(path_value) = effective_env_value(env, "PATH") else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{context} not found on PATH: {}", program.to_string_lossy()),
        ));
    };
    let pathext = effective_env_value(env, "PATHEXT");
    let extensions = executable_extensions(program, pathext.as_deref());
    let current_dir = current_dir
        .map(Path::to_owned)
        .or_else(|| env::current_dir().ok());
    for directory in env::split_paths(&path_value) {
        let directory = if directory.is_absolute() {
            directory
        } else if let Some(current_dir) = &current_dir {
            current_dir.join(directory)
        } else {
            directory
        };
        for extension in &extensions {
            let candidate = append_extension(&directory.join(program), extension);
            if let Ok(Some(path)) = resolve_exact_application_candidate(&candidate) {
                return Ok(path);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("{context} not found on PATH: {}", program.to_string_lossy()),
    ))
}

/// True when the program names a directory, so `PATH` must not be searched.
pub(crate) fn has_path_component(path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty())
}

fn resolve_application_candidate(
    path: &Path,
    pathext: Option<&OsStr>,
) -> io::Result<Option<PathBuf>> {
    for extension in executable_extensions(path, pathext) {
        if let Some(path) =
            resolve_exact_application_candidate(&append_extension(path, &extension))?
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn resolve_exact_application_candidate(path: &Path) -> io::Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    if !is_direct_application_path(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "executable must be .exe or .com: {}",
                path.to_string_lossy()
            ),
        ));
    }
    if path.is_absolute() {
        Ok(Some(path.to_owned()))
    } else {
        Ok(Some(env::current_dir()?.join(path)))
    }
}

fn executable_extensions(program: &Path, pathext: Option<&OsStr>) -> Vec<OsString> {
    if program.extension().is_some() {
        return vec![OsString::new()];
    }
    let mut extensions = vec![OsString::new()];
    append_direct_application_extensions(&mut extensions, pathext);
    extensions
}

fn append_direct_application_extensions(extensions: &mut Vec<OsString>, pathext: Option<&OsStr>) {
    let Some(pathext) = pathext else {
        extensions.push(OsString::from(".COM"));
        extensions.push(OsString::from(".EXE"));
        return;
    };
    let mut segment = [0_u16; 4];
    let mut length = 0_usize;
    let mut overflow = false;
    let mut saw_nonempty = false;
    for unit in pathext.encode_wide() {
        if unit == b';' as u16 {
            push_direct_application_extension(extensions, &segment[..length], overflow);
            length = 0;
            overflow = false;
        } else {
            saw_nonempty = true;
            if length < segment.len() {
                segment[length] = unit;
                length += 1;
            } else {
                overflow = true;
            }
        }
    }
    push_direct_application_extension(extensions, &segment[..length], overflow);
    if !saw_nonempty {
        extensions.push(OsString::from(".COM"));
        extensions.push(OsString::from(".EXE"));
    }
}

fn push_direct_application_extension(
    extensions: &mut Vec<OsString>,
    units: &[u16],
    overflow: bool,
) {
    if overflow {
        return;
    }
    match classify_exe_or_com(units, true) {
        Some(DirectApplicationExtension::Exe) => extensions.push(OsString::from(".EXE")),
        Some(DirectApplicationExtension::Com) => extensions.push(OsString::from(".COM")),
        None => {}
    }
}

fn append_extension(path: &Path, extension: &OsStr) -> PathBuf {
    let mut candidate = path.as_os_str().to_owned();
    candidate.push(extension);
    PathBuf::from(candidate)
}

fn is_direct_application_path(path: &Path) -> bool {
    path.extension()
        .map(|extension| is_exe_or_com(extension, false))
        .unwrap_or(true)
}

fn is_exe_or_com(value: &OsStr, allow_leading_dot: bool) -> bool {
    let mut buffered = [0_u16; 4];
    let mut length = 0;
    for unit in value.encode_wide() {
        if length == buffered.len() {
            return false;
        }
        buffered[length] = unit;
        length += 1;
    }
    classify_exe_or_com(&buffered[..length], allow_leading_dot).is_some()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirectApplicationExtension {
    Exe,
    Com,
}

fn classify_exe_or_com(
    units: &[u16],
    allow_leading_dot: bool,
) -> Option<DirectApplicationExtension> {
    let units = if allow_leading_dot && units.first() == Some(&(b'.' as u16)) {
        &units[1..]
    } else {
        units
    };
    if units.len() != 3 {
        return None;
    }
    match (
        ascii_lower(units[0]),
        ascii_lower(units[1]),
        ascii_lower(units[2]),
    ) {
        (b'e', b'x', b'e') => Some(DirectApplicationExtension::Exe),
        (b'c', b'o', b'm') => Some(DirectApplicationExtension::Com),
        _ => None,
    }
}

const fn ascii_lower(unit: u16) -> u8 {
    if unit >= b'A' as u16 && unit <= b'Z' as u16 {
        (unit + (b'a' - b'A') as u16) as u8
    } else if unit <= u8::MAX as u16 {
        unit as u8
    } else {
        0
    }
}

/// Last override for `name` wins; an override with no value removes the
/// variable instead of falling through to the parent environment.
fn effective_env_value(env: &[(OsString, Option<OsString>)], name: &str) -> Option<OsString> {
    env.iter()
        .rev()
        .find(|(key, _)| os_str_eq_ascii_ignore_case(key, name))
        .map_or_else(|| env::var_os(name), |(_, value)| value.clone())
}

fn os_str_eq_ascii_ignore_case(value: &OsStr, ascii: &str) -> bool {
    let mut units = value.encode_wide();
    for byte in ascii.bytes() {
        if units.next().map(ascii_lower) != Some(ascii_lower(byte as u16)) {
            return false;
        }
    }
    units.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStringExt as _;

    #[test]
    fn direct_application_extension_is_exact_ascii_and_allocation_free() {
        for extension in ["exe", "EXE", "com", "CoM"] {
            assert!(is_exe_or_com(OsStr::new(extension), false));
        }
        for extension in [".exe", ".EXE", ".com", ".CoM"] {
            assert!(is_exe_or_com(OsStr::new(extension), true));
        }
        for extension in ["", ".", "ex", "exe2", "..exe", ".bat", " exe"] {
            assert!(!is_exe_or_com(OsStr::new(extension), true));
        }
        let non_unicode = OsString::from_wide(&[b'e' as u16, 0xd800, b'e' as u16]);
        assert!(!is_exe_or_com(&non_unicode, false));
    }

    #[test]
    fn pathext_stream_preserves_fallback_order_and_filters_without_text_conversion() {
        let extensions = executable_extensions(
            Path::new("tool"),
            Some(OsStr::new(".BAT;EXE;.com;;.EXE;too-long")),
        );
        assert_eq!(
            extensions,
            ["", ".EXE", ".COM", ".EXE"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            executable_extensions(Path::new("tool"), Some(OsStr::new(";;;"))),
            ["", ".COM", ".EXE"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            executable_extensions(Path::new("tool"), Some(OsStr::new(".BAT"))),
            vec![OsString::new()]
        );
        assert!(os_str_eq_ascii_ignore_case(
            OsStr::new("PathExt"),
            "PATHEXT"
        ));
        assert!(!os_str_eq_ascii_ignore_case(
            OsStr::new("PATHEXT2"),
            "PATHEXT"
        ));
    }

    #[test]
    fn a_removing_override_hides_the_parent_path_value() {
        let env = vec![(OsString::from("PATH"), None)];
        assert_eq!(effective_env_value(&env, "PATH"), None);
        assert!(
            resolve_application_path(Path::new("definitely-not-on-path"), "probe", None, &env)
                .is_err()
        );
    }

    #[test]
    fn a_path_with_a_directory_is_never_searched_on_path() {
        assert!(has_path_component(Path::new("sub\\tool.exe")));
        assert!(has_path_component(Path::new("./tool.exe")));
        assert!(!has_path_component(Path::new("tool.exe")));
    }
}
