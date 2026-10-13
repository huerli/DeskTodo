//! Background CLI tools must not create a console window in the Windows GUI.
use std::ffi::OsStr;
use std::process::Command;

pub(crate) fn background_command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(windows)]
    let mut command = Command::new(program);
    #[cfg(not(windows))]
    let command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn background_child_has_no_console_and_keeps_output() {
        let output = background_command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", r#"
                Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class ConsoleProbe { [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); }';
                [ConsoleProbe]::GetConsoleWindow().ToInt64()
            "#])
            .output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0");
    }
}
