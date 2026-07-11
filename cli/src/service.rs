//! `ctx service` — install the CtxOne Hub as a login/boot service.
//!
//! Runs the unified daemon (`ctxone-hub --http --lens`) at login so it owns the
//! db *before* any agent starts. That's the fix for the reboot race: whoever
//! grabs the db lockfile first wins, and a supervised service always wins over
//! an agent-spawned stdio hub. See docs/DEPLOYMENT.md.
//!
//! Platform back-ends: macOS launchd LaunchAgent, Linux systemd *user* unit.
//! Generation is pure and unit-tested; `install`/`uninstall` additionally shell
//! out to `launchctl` / `systemctl --user`. Use `--dry-run` to print the unit
//! and the commands without touching the system.

use std::path::PathBuf;

/// launchd label / systemd unit stem. Also the plist filename stem.
pub const SERVICE_LABEL: &str = "com.ctxone.hub";
/// systemd unit file name.
pub const SYSTEMD_UNIT: &str = "ctxone-hub.service";
/// Windows Task Scheduler task name.
pub const WINDOWS_TASK: &str = "CtxOneHub";

/// Everything needed to render a service unit.
pub struct ServiceSpec {
    pub hub_bin: String,
    pub db_path: String,
    pub port: u16,
    pub lens: bool,
    pub auth_token: Option<String>,
    pub log_path: String,
}

impl ServiceSpec {
    /// The `ctxone-hub` argv the service launches.
    pub fn program_args(&self) -> Vec<String> {
        let mut args = vec!["--http".to_string()];
        if self.lens {
            args.push("--lens".to_string());
        }
        args.push("--path".to_string());
        args.push(self.db_path.clone());
        args.push("--port".to_string());
        args.push(self.port.to_string());
        args
    }

    /// macOS launchd plist.
    pub fn macos_plist(&self) -> String {
        let mut prog = format!(
            "    <key>ProgramArguments</key>\n    <array>\n      <string>{}</string>\n",
            xml_escape(&self.hub_bin)
        );
        for a in self.program_args() {
            prog.push_str(&format!("      <string>{}</string>\n", xml_escape(&a)));
        }
        prog.push_str("    </array>\n");

        let env = match &self.auth_token {
            Some(tok) => format!(
                "    <key>EnvironmentVariables</key>\n    <dict>\n      \
                 <key>CTXONE_AUTH_TOKEN</key>\n      <string>{}</string>\n    </dict>\n",
                xml_escape(tok)
            ),
            None => String::new(),
        };

        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \x20   <key>Label</key>\n    <string>{label}</string>\n\
             {prog}\
             {env}\
             \x20   <key>RunAtLoad</key>\n    <true/>\n\
             \x20   <key>KeepAlive</key>\n    <true/>\n\
             \x20   <key>StandardOutPath</key>\n    <string>{log}</string>\n\
             \x20   <key>StandardErrorPath</key>\n    <string>{log}</string>\n\
             </dict>\n\
             </plist>\n",
            label = xml_escape(SERVICE_LABEL),
            prog = prog,
            env = env,
            log = xml_escape(&self.log_path),
        )
    }

    /// Windows Task Scheduler definition (registered via `schtasks /xml`). A
    /// logon trigger starts the daemon at sign-in; RestartOnFailure gives it
    /// KeepAlive-like behaviour. Env vars (auth token) aren't expressible in
    /// this schema — set CTXONE_AUTH_TOKEN as a user env var instead.
    pub fn windows_task_xml(&self) -> String {
        let args = self
            .program_args()
            .iter()
            .map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() })
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
             <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\
             \x20 <RegistrationInfo>\n    <Description>CtxOne Hub (MCP + REST + Lens)</Description>\n  </RegistrationInfo>\n\
             \x20 <Triggers>\n    <LogonTrigger>\n      <Enabled>true</Enabled>\n    </LogonTrigger>\n  </Triggers>\n\
             \x20 <Principals>\n    <Principal id=\"Author\">\n      <LogonType>InteractiveToken</LogonType>\n      <RunLevel>LeastPrivilege</RunLevel>\n    </Principal>\n  </Principals>\n\
             \x20 <Settings>\n    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n    <RestartOnFailure>\n      <Interval>PT1M</Interval>\n      <Count>3</Count>\n    </RestartOnFailure>\n  </Settings>\n\
             \x20 <Actions Context=\"Author\">\n    <Exec>\n      <Command>{cmd}</Command>\n      <Arguments>{args}</Arguments>\n    </Exec>\n  </Actions>\n\
             </Task>\n",
            cmd = xml_escape(&self.hub_bin),
            args = xml_escape(&args),
        )
    }

    /// Linux systemd user unit.
    pub fn linux_unit(&self) -> String {
        let exec = std::iter::once(self.hub_bin.clone())
            .chain(self.program_args())
            .collect::<Vec<_>>()
            .join(" ");
        let env = match &self.auth_token {
            Some(tok) => format!("Environment=CTXONE_AUTH_TOKEN={tok}\n"),
            None => String::new(),
        };
        format!(
            "[Unit]\n\
             Description=CtxOne Hub (MCP + REST + Lens)\n\
             After=network.target\n\
             \n\
             [Service]\n\
             ExecStart={exec}\n\
             {env}\
             Restart=on-failure\n\
             RestartSec=3\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
        )
    }
}

/// Minimal XML escaping for plist string values.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Where the unit file is written for the current OS. `None` on unsupported
/// platforms (e.g. Windows — no boot-service back-end here yet).
pub fn service_file_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        dirs::home_dir()
            .map(|h| h.join("Library/LaunchAgents").join(format!("{SERVICE_LABEL}.plist")))
    } else if cfg!(target_os = "linux") {
        dirs::config_dir().map(|c| c.join("systemd/user").join(SYSTEMD_UNIT))
    } else if cfg!(target_os = "windows") {
        // The task lives in Task Scheduler; this is where we stash the XML that
        // `schtasks /create /xml` reads from (and that `uninstall` removes).
        dirs::data_dir().map(|d| d.join("ctxone").join("ctxone-hub-task.xml"))
    } else {
        None
    }
}

/// Default log path for the service (`~/.ctxone/hub.log`).
pub fn default_log_path() -> String {
    dirs::home_dir()
        .map(|h| h.join(".ctxone/hub.log"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/tmp/ctxone-hub.log".to_string())
}

type CmdResult = Result<(), Box<dyn std::error::Error>>;

fn render(spec: &ServiceSpec) -> Result<String, Box<dyn std::error::Error>> {
    if cfg!(target_os = "macos") {
        Ok(spec.macos_plist())
    } else if cfg!(target_os = "linux") {
        Ok(spec.linux_unit())
    } else if cfg!(target_os = "windows") {
        Ok(spec.windows_task_xml())
    } else {
        Err("`ctx service` supports macOS (launchd), Linux (systemd), and Windows \
             (Task Scheduler). Run `ctxone-hub --http --lens` under your own \
             supervisor instead."
            .into())
    }
}

/// Write the unit file and register it with the platform supervisor. With
/// `dry_run`, print the unit + the commands and touch nothing.
pub fn install(spec: &ServiceSpec, dry_run: bool, force: bool) -> CmdResult {
    let content = render(spec)?;
    let path = service_file_path().ok_or("unsupported platform")?;

    if dry_run {
        println!("[dry-run] would write {}\n", path.display());
        println!("{content}");
        println!("[dry-run] would then run:");
        for (bin, args) in register_commands(&path) {
            println!("    {bin} {}", args.join(" "));
        }
        return Ok(());
    }

    if path.exists() && !force {
        return Err(format!(
            "{} already exists — pass --force to overwrite (or `ctx service uninstall` first)",
            path.display()
        )
        .into());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Ensure the log directory exists so the supervisor can open the log.
    if let Some(log_parent) = std::path::Path::new(&spec.log_path).parent() {
        let _ = std::fs::create_dir_all(log_parent);
    }
    std::fs::write(&path, &content)?;
    if spec.auth_token.is_some() {
        if cfg!(target_os = "windows") {
            // Task Scheduler XML can't carry env vars, so the token isn't in the
            // unit — point the user at a user-level env var instead.
            eprintln!(
                "  \u{26A0} Windows Task Scheduler can't embed env vars; the token was \
                 NOT written. Set it as a user env var: setx CTXONE_AUTH_TOKEN <token>"
            );
        } else {
            // A token embedded in the unit is a secret at rest — lock it down.
            set_owner_only(&path);
            eprintln!(
                "  \u{26A0} auth token written into {} (chmod 600). Anyone who can read \
                 that file can read the token.",
                path.display()
            );
        }
    }
    println!("\u{2192} wrote {}", path.display());

    for (bin, args) in register_commands(&path) {
        run(&bin, &args)?;
    }
    println!("\u{2713} service installed and started ({SERVICE_LABEL}).");
    println!("  logs:   {}", spec.log_path);
    println!("  status: ctx service status");
    println!(
        "  note: if another hub already holds {} (an agent's stdio hub, or a \
         manual one), the service will fail on the db lockfile — stop it first.",
        spec.db_path
    );
    Ok(())
}

/// Deregister and remove the unit file.
pub fn uninstall(dry_run: bool) -> CmdResult {
    let path = service_file_path().ok_or("unsupported platform")?;
    if dry_run {
        println!("[dry-run] would run:");
        for (bin, args) in deregister_commands(&path) {
            println!("    {bin} {}", args.join(" "));
        }
        println!("[dry-run] would remove {}", path.display());
        return Ok(());
    }
    for (bin, args) in deregister_commands(&path) {
        // Best-effort: the service may already be stopped/absent.
        let _ = run(&bin, &args);
    }
    if path.exists() {
        std::fs::remove_file(&path)?;
        println!("\u{2713} removed {}", path.display());
    } else {
        println!("(no service file at {})", path.display());
    }
    Ok(())
}

/// Show whether the platform supervisor knows about the service.
pub fn status() -> CmdResult {
    let (bin, args) = status_command();
    println!("$ {bin} {}", args.join(" "));
    let _ = std::process::Command::new(&bin).args(&args).status();
    if let Some(path) = service_file_path() {
        println!(
            "\nunit file: {} ({})",
            path.display(),
            if path.exists() { "present" } else { "absent" }
        );
    }
    Ok(())
}

/// Commands to load/enable the service after the unit file is written.
fn register_commands(path: &std::path::Path) -> Vec<(String, Vec<String>)> {
    let p = path.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") {
        vec![
            // Unload any stale copy first (ignored if not loaded), then load.
            ("launchctl".into(), vec!["unload".into(), p.clone()]),
            ("launchctl".into(), vec!["load".into(), "-w".into(), p]),
        ]
    } else if cfg!(target_os = "windows") {
        vec![(
            "schtasks".into(),
            vec![
                "/create".into(),
                "/tn".into(),
                WINDOWS_TASK.into(),
                "/xml".into(),
                p,
                "/f".into(),
            ],
        )]
    } else {
        vec![
            ("systemctl".into(), vec!["--user".into(), "daemon-reload".into()]),
            (
                "systemctl".into(),
                vec!["--user".into(), "enable".into(), "--now".into(), SYSTEMD_UNIT.into()],
            ),
        ]
    }
}

fn deregister_commands(path: &std::path::Path) -> Vec<(String, Vec<String>)> {
    let p = path.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") {
        vec![("launchctl".into(), vec!["unload".into(), p])]
    } else if cfg!(target_os = "windows") {
        vec![(
            "schtasks".into(),
            vec!["/delete".into(), "/tn".into(), WINDOWS_TASK.into(), "/f".into()],
        )]
    } else {
        vec![(
            "systemctl".into(),
            vec!["--user".into(), "disable".into(), "--now".into(), SYSTEMD_UNIT.into()],
        )]
    }
}

fn status_command() -> (String, Vec<String>) {
    if cfg!(target_os = "macos") {
        ("launchctl".into(), vec!["list".into(), SERVICE_LABEL.into()])
    } else if cfg!(target_os = "windows") {
        (
            "schtasks".into(),
            vec!["/query".into(), "/tn".into(), WINDOWS_TASK.into()],
        )
    } else {
        (
            "systemctl".into(),
            vec!["--user".into(), "status".into(), SYSTEMD_UNIT.into()],
        )
    }
}

/// Run a command, mapping a non-zero exit into an error.
fn run(bin: &str, args: &[String]) -> CmdResult {
    let status = std::process::Command::new(bin).args(args).status()?;
    if !status.success() {
        return Err(format!("`{bin} {}` failed ({status})", args.join(" ")).into());
    }
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(auth: Option<&str>, lens: bool) -> ServiceSpec {
        ServiceSpec {
            hub_bin: "/opt/homebrew/bin/ctxone-hub".into(),
            db_path: "/Users/user/.ctxone/memory.db".into(),
            port: 3001,
            lens,
            auth_token: auth.map(str::to_string),
            log_path: "/Users/user/.ctxone/hub.log".into(),
        }
    }

    #[test]
    fn program_args_includes_http_and_path_and_port() {
        let a = spec(None, true).program_args();
        assert_eq!(
            a,
            vec![
                "--http",
                "--lens",
                "--path",
                "/Users/user/.ctxone/memory.db",
                "--port",
                "3001"
            ]
        );
    }

    #[test]
    fn no_lens_omits_lens_flag() {
        let a = spec(None, false).program_args();
        assert!(!a.contains(&"--lens".to_string()));
        assert!(a.contains(&"--http".to_string()));
    }

    #[test]
    fn macos_plist_has_label_runatload_and_args() {
        let p = spec(None, true).macos_plist();
        assert!(p.contains("<string>com.ctxone.hub</string>"));
        assert!(p.contains("<key>RunAtLoad</key>"));
        assert!(p.contains("<string>--lens</string>"));
        assert!(p.contains("<string>/opt/homebrew/bin/ctxone-hub</string>"));
        // No token → no EnvironmentVariables block.
        assert!(!p.contains("CTXONE_AUTH_TOKEN"));
    }

    #[test]
    fn macos_plist_embeds_token_when_set() {
        let p = spec(Some("s3cret"), true).macos_plist();
        assert!(p.contains("<key>CTXONE_AUTH_TOKEN</key>"));
        assert!(p.contains("<string>s3cret</string>"));
    }

    #[test]
    fn linux_unit_has_execstart_and_wantedby() {
        let u = spec(None, false).linux_unit();
        assert!(u.contains("ExecStart=/opt/homebrew/bin/ctxone-hub --http --path"));
        assert!(u.contains("WantedBy=default.target"));
        assert!(u.contains("Restart=on-failure"));
        assert!(!u.contains("--lens"));
    }

    #[test]
    fn linux_unit_sets_token_env() {
        let u = spec(Some("s3cret"), true).linux_unit();
        assert!(u.contains("Environment=CTXONE_AUTH_TOKEN=s3cret"));
    }

    #[test]
    fn windows_task_xml_has_logon_trigger_and_command() {
        let x = spec(None, true).windows_task_xml();
        assert!(x.contains("<LogonTrigger>"));
        assert!(x.contains("<Command>/opt/homebrew/bin/ctxone-hub</Command>"));
        assert!(x.contains("--http --lens --path"));
        assert!(x.contains("<RestartOnFailure>"));
    }

    #[test]
    fn windows_task_xml_quotes_spaced_path() {
        let mut s = spec(None, false);
        s.db_path = "C:\\Users\\Ada Lovelace\\.ctxone\\memory.db".into();
        let x = s.windows_task_xml();
        // A path with a space must be quoted inside the single Arguments string.
        assert!(x.contains("\"C:\\Users\\Ada Lovelace\\.ctxone\\memory.db\""));
    }

    #[test]
    fn windows_task_xml_omits_token() {
        // Env vars aren't expressible in this schema; the token must not leak in.
        let x = spec(Some("s3cret"), true).windows_task_xml();
        assert!(!x.contains("s3cret"));
    }
}
