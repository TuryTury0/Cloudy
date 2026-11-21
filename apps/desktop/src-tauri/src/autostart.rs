//! Starting with Windows: a value under HKCU\Software\Microsoft\Windows\CurrentVersion\Run, so AnyTask runs at the
//! user's sign-in without being opened (the phone finds it then). Only the current user's own key: no admin rights,
//! nothing system-wide. It runs after sign-in only: before it, no app runs at all.

const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "AnyTask";

/// The reg.exe arguments that add or remove the value for `exe`.
pub fn command(enabled: bool, exe: &str) -> Vec<String> {
    if enabled {
        vec!["add".into(), KEY.into(), "/v".into(), VALUE.into(), "/t".into(), "REG_SZ".into(), "/d".into(), format!("\"{exe}\""), "/f".into()]
    } else {
        vec!["delete".into(), KEY.into(), "/v".into(), VALUE.into(), "/f".into()]
    }
}

/// Adds or removes the start-with-Windows value for this program. Removing a value that is not there is fine.
pub fn set(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut run = std::process::Command::new("reg");
    run.args(command(enabled, &exe.to_string_lossy()));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        run.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let status = run.status().map_err(|e| format!("Could not run reg.exe: {e}"))?;
    if status.success() || !enabled { Ok(()) } else { Err(format!("Windows did not save the start-up entry ({status}).")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_value_is_this_users_own_and_quoted() {
        let add = command(true, r"C:\Users\x\AppData\Local\AnyTask\anytask-desktop.exe");
        assert_eq!(add[0], "add");
        assert!(add[1].starts_with(r"HKCU\"), "the current user's key only");
        assert!(add.contains(&r#""C:\Users\x\AppData\Local\AnyTask\anytask-desktop.exe""#.to_string()), "a path with spaces stays one path");
        assert_eq!(command(false, "x")[0], "delete");
    }
}
