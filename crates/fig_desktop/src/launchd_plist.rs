//! The `~/Library/LaunchAgents` plist that starts the desktop app at login.
//!
//! This was `fig_util::launchd_plist` — 359 lines in the crate every binary links, for the single
//! caller below it (`install.rs`). `fig_util` is the shared-name-and-path crate; a plist writer
//! for one consumer is not shared code.
//!
//! Moving it also made its dead surface visible to `dead_code`. Upstream had a builder method per
//! launchd key it had ever wanted — `program`, `environment_variables`,
//! `standard_{in,out,error}_path`, `working_directory`, `keep_alive`, `throttle_interval` — plus a
//! `watch_paths` field with no setter at all, so it could never be written. The one plist this app
//! writes sets four keys, and that is what this builder now offers.

use std::fmt::Write;

use camino::Utf8PathBuf;

#[derive(Debug, Clone, Default)]
pub struct LaunchdPlist {
    label: String,
    program_arguments: Option<Vec<String>>,
    run_at_load: Option<bool>,
    associated_bundle_identifiers: Option<Vec<String>>,
}

impl LaunchdPlist {
    pub fn new(label: impl Into<String>) -> LaunchdPlist {
        LaunchdPlist {
            label: label.into(),
            ..LaunchdPlist::default()
        }
    }

    pub fn get_file_path(&self) -> Result<Utf8PathBuf, fig_util::Error> {
        let path = fig_util::directories::home_dir_utf8()?
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", self.label));

        Ok(path)
    }

    /// The argv the agent runs, `argv[0]` first.
    pub fn program_arguments<I, T>(mut self, program_arguments: I) -> LaunchdPlist
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        self.program_arguments = Some(program_arguments.into_iter().map(|s| s.into()).collect());
        self
    }

    /// Start the job when the agent is loaded (i.e. at login).
    pub fn run_at_load(mut self, run_at_load: bool) -> LaunchdPlist {
        self.run_at_load = Some(run_at_load);
        self
    }

    /// The bundle ids this agent belongs to, so macOS attributes it to the app in
    /// System Settings ▸ General ▸ Login Items rather than listing it as an unknown developer.
    pub fn associated_bundle_identifiers<I, T>(mut self, associated_bundle_identifiers: I) -> LaunchdPlist
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        self.associated_bundle_identifiers =
            Some(associated_bundle_identifiers.into_iter().map(|s| s.into()).collect());
        self
    }

    /// Generate the plist as a string
    fn plist(&self) -> String {
        let mut plist = String::new();

        let indent = "    ";
        let mut indent_level = 0;

        macro_rules! indent_block {
            ($block:block) => {{
                {
                    indent_level += 1;
                    $block;
                    indent_level -= 1;
                }
            }};
        }

        macro_rules! push_line {
            ($($arg:tt)*) => {{
                for _ in 0..indent_level {
                    plist.push_str(indent);
                }
                writeln!(plist, $($arg)*).ok();
            }};
        }

        macro_rules! push_key_val {
            ($key:expr,String, $val:expr) => {{
                push_line!("<key>{}</key>", $key);
                push_line!("<string>{}</string>", $val);
            }};
            ($key:expr, &[String], $val:expr) => {{
                push_line!("<key>{}</key>", $key);
                push_line!("<array>");
                indent_block!({
                    for s in $val.iter() {
                        push_line!("<string>{}</string>", s);
                    }
                });
                push_line!("</array>");
            }};
            ($key:expr,bool, $val:expr) => {{
                push_line!("<key>{}</key>", $key);
                push_line!("{}", if $val { "<true/>" } else { "<false/>" });
            }};
        }

        push_line!(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        push_line!(
            r#"<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#
        );
        push_line!(r#"<plist version="1.0">"#);

        indent_block!({
            push_line!("<dict>");

            indent_block!({
                push_key_val!("Label", String, &self.label);

                if let Some(program_arguments) = &self.program_arguments {
                    push_key_val!("ProgramArguments", &[String], program_arguments);
                }

                if let Some(run_at_load) = &self.run_at_load {
                    push_key_val!("RunAtLoad", bool, *run_at_load);
                }

                if let Some(associated_bundle_identifiers) = &self.associated_bundle_identifiers {
                    push_key_val!("AssociatedBundleIdentifiers", &[String], associated_bundle_identifiers);
                }
            });

            push_line!("</dict>");
        });

        push_line!("</plist>");

        plist
    }
}

pub fn create_launch_agent(launchd_plist: &LaunchdPlist) -> Result<(), fig_util::Error> {
    let path = launchd_plist.get_file_path()?;
    let contents = launchd_plist.plist();
    if let Some(parent_dir) = path.parent() {
        std::fs::create_dir_all(parent_dir)?;
    }
    std::fs::write(&path, contents.as_bytes())?;

    Ok(())
}

#[cfg(test)]
mod test {
    use fig_util::consts::APP_BUNDLE_ID;

    use super::*;

    /// The exact plist `install.rs` writes, spelled out.
    #[test]
    fn test_plist() {
        let plist = LaunchdPlist::new(format!("{APP_BUNDLE_ID}.test"))
            .program_arguments(["/Applications/Fig.app/Contents/MacOS/fig_desktop", "--is-startup"])
            .run_at_load(true)
            .associated_bundle_identifiers([APP_BUNDLE_ID])
            .plist();

        let valid_plist = indoc::formatdoc! {"
            <?xml version=\"1.0\" encoding=\"UTF-8\"?>
            <!DOCTYPE plist PUBLIC \"-//Apple Computer//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
            <plist version=\"1.0\">
                <dict>
                    <key>Label</key>
                    <string>{APP_BUNDLE_ID}.test</string>
                    <key>ProgramArguments</key>
                    <array>
                        <string>/Applications/Fig.app/Contents/MacOS/fig_desktop</string>
                        <string>--is-startup</string>
                    </array>
                    <key>RunAtLoad</key>
                    <true/>
                    <key>AssociatedBundleIdentifiers</key>
                    <array>
                        <string>{APP_BUNDLE_ID}</string>
                    </array>
                </dict>
            </plist>
        "};

        assert_eq!(plist, valid_plist);
    }

    /// `plutil -lint` has to accept it — a malformed login agent fails silently at login.
    #[test]
    fn plist_is_valid_xml_plist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.plist");
        let plist = LaunchdPlist::new(format!("{APP_BUNDLE_ID}.test"))
            .program_arguments(["/bin/true"])
            .run_at_load(true)
            .associated_bundle_identifiers([APP_BUNDLE_ID])
            .plist();
        std::fs::write(&path, &plist).unwrap();

        let out = std::process::Command::new("plutil")
            .arg("-lint")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "plutil -lint rejected the plist: {}{}\n{plist}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
}
