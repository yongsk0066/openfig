use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::LazyLock;

use async_trait::async_trait;
use clap::ValueEnum;
use fig_os_shim::Env;
use fig_util::{
    CLI_BINARY_NAME,
    HOOK_PROTOCOL_VERSION,
    PRODUCT_NAME,
    Shell,
    directories,
};
use regex::Regex;
use serde::{
    Deserialize,
    Serialize,
};

use crate::atomic::write_atomic;
use crate::error::{
    ErrorExt,
    Result,
};
use crate::{
    Error,
    FileIntegration,
    Integration,
    backup_file,
};

/// The oldest zsh the hook loads in: `zle-line-pre-redraw` and `add-zle-hook-widget` arrived in
/// 5.3. Older shells get no autocomplete and are otherwise untouched.
pub const ZSH_HOOK_MIN_VERSION: &str = "5.3";

/// `scripts/fig.zsh` ready to be `eval`ed: socket path and protocol version filled in. It is the
/// whole of zsh's `post` integration (docs/15-architecture-study.md §3.3).
fn zsh_hook_source() -> Result<String> {
    let socket = directories::hook_socket_path()?;
    // The path goes between single quotes.
    let socket = socket.to_string_lossy().replace('\'', r"'\''");
    Ok(include_str!("scripts/fig.zsh")
        .replace("{{PRODUCT_NAME}}", PRODUCT_NAME)
        .replace("{{HOOK_SOCKET}}", &socket)
        .replace("{{HOOK_PROTOCOL_VERSION}}", &HOOK_PROTOCOL_VERSION.to_string())
        .replace("{{ZSH_HOOK_MIN_VERSION}}", ZSH_HOOK_MIN_VERSION))
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    Pre,
    Post,
}

impl When {
    fn all() -> [When; 2] {
        [Self::Pre, Self::Post]
    }
}

impl std::fmt::Display for When {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            When::Pre => write!(f, "pre"),
            When::Post => write!(f, "post"),
        }
    }
}

/// Leading `.` of a dotfile name, e.g. `.zshrc` -> `zshrc`.
static LEADING_DOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\.").unwrap());

/// A `# fig ignore` marker anywhere in an rc file.
static FIG_IGNORE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?mi)^\s*#\s*fig ignore\s?.*$").unwrap());

/// Comment-only and empty lines.
static COMMENT_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*(#.*)?\n").unwrap());

fn integration_file_name(dotfile_name: &str, when: &When, shell: &Shell) -> String {
    format!("{}.{when}.{shell}", LEADING_DOT.replace_all(dotfile_name, ""))
}

pub trait ShellExt {
    fn get_shell_integrations(&self, env: &Env) -> Result<Vec<Box<dyn ShellIntegration>>>;
    /// Script integrations are installed into `<data dir>/shell`
    fn get_script_integrations(&self) -> Result<Vec<ShellScriptShellIntegration>>;
    fn get_fig_integration_source(&self, when: &When) -> Result<String>;
}

impl ShellExt for Shell {
    fn get_script_integrations(&self) -> Result<Vec<ShellScriptShellIntegration>> {
        let mut integrations = vec![];

        for file in match self {
            Shell::Bash => [".bashrc", ".bash_profile", ".bash_login", ".profile"].iter(),
            Shell::Zsh => [".zshrc", ".zprofile"].iter(),
        } {
            for when in &When::all() {
                let path = directories::fig_data_dir()?
                    .join("shell")
                    .join(integration_file_name(file, when, self));

                integrations.push(ShellScriptShellIntegration {
                    shell: *self,
                    when: *when,
                    path,
                });
            }
        }

        Ok(integrations)
    }

    fn get_shell_integrations(&self, env: &Env) -> Result<Vec<Box<dyn ShellIntegration>>> {
        let config_dir = self.get_config_directory(env)?;

        let integrations: Vec<Box<dyn ShellIntegration>> = match self {
            Shell::Bash => {
                let mut configs = vec![".bashrc"];
                let other_configs = [".profile", ".bash_login", ".bash_profile"];

                configs.extend(other_configs.into_iter().filter(|f| config_dir.join(f).exists()));

                // Include .profile if none of [.profile, .bash_login, .bash_profile] exist.
                if configs.len() == 1 {
                    configs.push(other_configs[0]);
                }

                configs
                    .into_iter()
                    .map(|filename| {
                        Box::new(DotfileShellIntegration {
                            pre: true,
                            post: true,
                            shell: *self,
                            dotfile_directory: config_dir.clone(),
                            dotfile_name: filename,
                        }) as Box<dyn ShellIntegration>
                    })
                    .collect()
            },
            Shell::Zsh => vec![".zshrc", ".zprofile"]
                .into_iter()
                .map(|filename| {
                    Box::new(DotfileShellIntegration {
                        pre: true,
                        post: true,
                        shell: *self,
                        dotfile_directory: config_dir.clone(),
                        dotfile_name: filename,
                    }) as Box<dyn ShellIntegration>
                })
                .collect(),
        };

        Ok(integrations)
    }

    fn get_fig_integration_source(&self, when: &When) -> Result<String> {
        let script = match (self, when) {
            (Shell::Zsh, When::Pre) => include_str!("scripts/pre.zsh"),
            (Shell::Zsh, When::Post) => return zsh_hook_source(),
            (Shell::Bash, When::Pre) => {
                concat!(
                    "function __fig_source_bash_preexec() {\n",
                    include_str!("scripts/bash-preexec.sh"),
                    "}\n",
                    "__fig_source_bash_preexec\n",
                    "# shellcheck disable=SC2329\n",
                    "function __bp_adjust_histcontrol() { :; }\n",
                    include_str!("scripts/pre.sh")
                )
            },
            (Shell::Bash, When::Post) => {
                concat!(
                    "function __fig_source_bash_preexec() {\n",
                    include_str!("scripts/bash-preexec.sh"),
                    "}\n",
                    "__fig_source_bash_preexec\n",
                    "# shellcheck disable=SC2329\n",
                    "function __bp_adjust_histcontrol() { :; }\n",
                    include_str!("scripts/post.bash")
                )
            },
        };

        Ok(script.replace("{{CLI_BINARY_NAME}}", CLI_BINARY_NAME))
    }
}

pub trait ShellIntegration: Send + Sync + Integration + ShellIntegrationClone {
    // The unique name of the integration file
    fn file_name(&self) -> &str;
    fn get_shell(&self) -> Shell;
    fn path(&self) -> PathBuf;
}

impl std::fmt::Display for dyn ShellIntegration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.get_shell(), self.path().display())
    }
}

pub trait ShellIntegrationClone {
    fn clone_box(&self) -> Box<dyn ShellIntegration>;
}

impl<T> ShellIntegrationClone for T
where
    T: 'static + ShellIntegration + Clone,
{
    fn clone_box(&self) -> Box<dyn ShellIntegration> {
        Box::new(self.clone())
    }
}

// We can now implement Clone manually by forwarding to clone_box.
impl Clone for Box<dyn ShellIntegration> {
    fn clone(&self) -> Box<dyn ShellIntegration> {
        self.clone_box()
    }
}

#[derive(Debug, Clone)]
pub struct ShellScriptShellIntegration {
    shell: Shell,
    when: When,
    path: PathBuf,
}

fn get_prefix(s: &str) -> &str {
    match s.find('.') {
        Some(i) => &s[..i],
        None => s,
    }
}

impl ShellScriptShellIntegration {
    fn get_file_integration(&self) -> FileIntegration {
        FileIntegration {
            path: self.path.clone(),
            contents: self.get_contents(),
            mode: None,
        }
    }

    fn get_name(&self) -> Option<&str> {
        self.path.file_name().and_then(|s| s.to_str())
    }

    fn get_contents(&self) -> String {
        let Self { shell, when, path } = self;
        let rcfile = match path.file_name().and_then(|x| x.to_str()) {
            Some(name) => format!(" --rcfile {}", get_prefix(name)),
            None => "".into(),
        };
        match self.shell {
            // Check if ~/.local/bin/{CLI_BINARY_NAME} is executable before eval
            Shell::Bash | Shell::Zsh => format!(
                "[ -x ~/.local/bin/{CLI_BINARY_NAME} ] && eval \"$(~/.local/bin/{CLI_BINARY_NAME} init {shell} {when}{rcfile})\""
            ),
        }
    }
}

#[async_trait]
impl Integration for ShellScriptShellIntegration {
    async fn is_installed(&self) -> Result<()> {
        self.get_file_integration().is_installed().await
    }

    async fn install(&self) -> Result<()> {
        self.get_file_integration().install().await
    }

    async fn uninstall(&self) -> Result<()> {
        self.get_file_integration().uninstall().await
    }

    fn describe(&self) -> String {
        format!("{} {}", self.shell, self.when)
    }
}

impl ShellIntegration for ShellScriptShellIntegration {
    fn file_name(&self) -> &str {
        self.get_name().unwrap_or("unknown_script")
    }

    fn get_shell(&self) -> Shell {
        self.shell
    }

    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}

/// zsh and bash integration where we modify a dotfile with pre/post hooks that reference script
/// files.
#[derive(Debug, Clone)]
pub struct DotfileShellIntegration {
    shell: Shell,
    pre: bool,
    post: bool,
    dotfile_directory: PathBuf,
    dotfile_name: &'static str,
}

impl DotfileShellIntegration {
    fn dotfile_path(&self) -> PathBuf {
        self.dotfile_directory.join(self.dotfile_name)
    }

    fn script_integration(&self, when: When) -> Result<ShellScriptShellIntegration> {
        let integration_file_name = format!(
            "{}.{}.{}",
            LEADING_DOT.replace_all(self.dotfile_name, ""),
            when,
            self.shell
        );
        Ok(ShellScriptShellIntegration {
            shell: self.shell,
            when,
            path: directories::fig_data_dir()?.join("shell").join(integration_file_name),
        })
    }

    #[allow(clippy::unused_self)]
    fn description(&self, when: When) -> String {
        match when {
            When::Pre => format!("# {PRODUCT_NAME} pre block. Keep at the top of this file."),
            When::Post => format!("# {PRODUCT_NAME} post block. Keep at the bottom of this file."),
        }
    }

    fn source_text(&self, when: When) -> Result<String> {
        let home = directories::home_dir()?;
        let integration_path = self.script_integration(when)?.path;
        let path = format!("\"${{HOME}}/{}\"", integration_path.strip_prefix(home)?.display());

        Ok(format!("[[ -f {path} ]] && builtin source {path}"))
    }

    fn source_regex(&self, when: When, constrain_position: bool) -> Result<Regex> {
        let regex = format!(
            r#"{}(?:{}\n)?{}\n{{0,2}}{}"#,
            if constrain_position && when == When::Pre {
                "^"
            } else {
                ""
            },
            regex::escape(&self.description(when)),
            regex::escape(&self.source_text(when)?),
            if constrain_position && when == When::Post {
                "$"
            } else {
                ""
            },
        );
        Ok(Regex::new(&regex)?)
    }

    fn remove_from_text(&self, text: impl Into<String>, when: When) -> Result<String> {
        let source_regex = self.source_regex(when, false)?;
        Ok(source_regex.replace_all(&text.into(), "").into())
    }

    fn matches_text(&self, text: &str, when: When) -> Result<()> {
        let dotfile = self.dotfile_path();
        if !self.source_regex(when, false)?.is_match(text) {
            let message = format!("{} does not source {} integration", dotfile.display(), when);
            return Err(Error::NotInstalled(message.into()));
        }
        if !self.source_regex(when, true)?.is_match(text) {
            let position = match when {
                When::Pre => "first",
                When::Post => "last",
            };
            let message = format!(
                "{} does not source {} integration {}",
                dotfile.display(),
                when,
                position
            );
            return Err(Error::ImproperInstallation(message.into()));
        }
        Ok(())
    }

    /// Whether either of our blocks is currently present in `text`.
    fn is_present_in(&self, text: &str) -> Result<bool> {
        Ok((self.pre && self.source_regex(When::Pre, false)?.is_match(text))
            || (self.post && self.source_regex(When::Post, false)?.is_match(text)))
    }

    /// The rc file with our blocks removed and the remainder normalised.
    ///
    /// Shared by `uninstall` and `install_inner` so that installing over an existing install and
    /// uninstalling agree byte for byte.
    fn strip_from_text(&self, text: &str) -> Result<String> {
        let mut contents = text.to_owned();

        if self.pre {
            contents = self.remove_from_text(&contents, When::Pre)?;
        }

        if self.post {
            contents = self.remove_from_text(&contents, When::Post)?;
        }

        let mut contents = contents.trim().to_string();
        contents.push('\n');
        Ok(contents)
    }

    /// The rc file as it should look once installed, given its current contents.
    fn compose(&self, existing: &str) -> Result<String> {
        let mut contents = self.strip_from_text(existing)?;

        if self.pre {
            let (shebang, post_shebang) = split_shebang(&contents);
            contents = format!(
                "{}{}\n{}\n{}",
                shebang,
                self.description(When::Pre),
                self.source_text(When::Pre)?,
                post_shebang,
            );
        }

        if self.post {
            contents = format!(
                "{}\n{}\n{}\n",
                contents,
                self.description(When::Post),
                self.source_text(When::Post)?,
            );
        }

        Ok(contents)
    }

    async fn install_inner(&self) -> Result<()> {
        let dotfile = self.dotfile_path();

        // This used to run a full `uninstall()` — a truncating rewrite of the rc file — and then
        // read the result back, so every install rewrote the user's rc file twice and the
        // "did anything change?" guard below compared the new contents against the *post-uninstall*
        // snapshot rather than against what was on disk. Strip in memory instead: one comparison
        // against the real original, and at most one write.
        let original_contents = if dotfile.exists() {
            backup_file(&dotfile, None::<PathBuf>)?;
            std::fs::read_to_string(&dotfile)?
        } else {
            String::new()
        };

        if self.pre {
            self.script_integration(When::Pre)?.install().await?;
        }

        if self.post {
            self.script_integration(When::Post)?.install().await?;
        }

        let contents = self.compose(&original_contents)?;

        if contents != original_contents {
            write_atomic(&dotfile, contents.as_bytes(), None).with_path(self.path())?;
        }
        Ok(())
    }
}

#[async_trait]
impl Integration for DotfileShellIntegration {
    fn describe(&self) -> String {
        format!(
            "{}{}{} into {}",
            self.shell,
            if self.pre { " pre" } else { "" },
            if self.post { " post" } else { "" },
            self.dotfile_name,
        )
    }

    async fn install(&self) -> Result<()> {
        if self.is_installed().await.is_ok() {
            return Ok(());
        }
        self.install_inner().await?;
        Ok(())
    }

    async fn uninstall(&self) -> Result<()> {
        let dotfile = self.dotfile_path();
        if dotfile.exists() {
            let original_contents = std::fs::read_to_string(&dotfile)?;

            // Do not rewrite a file we have nothing to remove from. This runs on rc files the
            // integration was never installed into (`uninstall` iterates every candidate dotfile
            // for both shells), and the rewrite alone trims the user's trailing whitespace.
            if self.is_present_in(&original_contents)? {
                let contents = self.strip_from_text(&original_contents)?;
                write_atomic(&dotfile, contents.as_bytes(), None).with_path(self.path())?;
            }
        }

        if self.pre {
            self.script_integration(When::Pre)?.uninstall().await?;
        }

        if self.post {
            self.script_integration(When::Post)?.uninstall().await?;
        }

        Ok(())
    }

    async fn is_installed(&self) -> Result<()> {
        let dotfile = self.dotfile_path();

        let filtered_contents: String = match std::fs::read_to_string(&dotfile).with_path(&dotfile) {
            // Remove comments and empty lines.
            Ok(contents) => {
                // Check for existence of ignore flag
                if FIG_IGNORE.is_match(&contents) {
                    return Ok(());
                }

                COMMENT_LINE.replace_all(&contents, "").into()
            },
            Err(Error::Io(err)) if err.kind() == ErrorKind::NotFound => {
                return Err(Error::FileDoesNotExist(dotfile.into()));
            },
            Err(err) => return Err(err),
        };

        let filtered_contents = filtered_contents.trim();

        if self.pre {
            self.matches_text(filtered_contents, When::Pre)?;
            self.script_integration(When::Pre)?.is_installed().await?;
        }

        if self.post {
            self.matches_text(filtered_contents, When::Post)?;
            self.script_integration(When::Post)?.is_installed().await?;
        }

        Ok(())
    }
}

impl ShellIntegration for DotfileShellIntegration {
    fn get_shell(&self) -> Shell {
        self.shell
    }

    fn path(&self) -> PathBuf {
        self.dotfile_path()
    }

    fn file_name(&self) -> &str {
        self.dotfile_name
    }
}

/// Splits the line containing the shebang (if any) with the rest of the string.
/// If the shebang exists, the newline is included. Otherwise, an empty slice is returned.
fn split_shebang(contents: &str) -> (&str, &str) {
    if contents.starts_with("#!") {
        match contents.find('\n') {
            Some(i) => (&contents[..i + 1], &contents[i + 1..]),
            None => ("", contents),
        }
    } else {
        ("", contents)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_split_shebang() {
        let shebang = "#!/usr/bin/env sh";
        let contents = "echo hello world";
        let with_shebang = format!("{}\n{}", shebang, contents);
        let with_shebang_no_lf = format!("{}{}", shebang, contents);
        let without_shebang = contents;
        assert_eq!(
            (format!("{shebang}\n").as_str(), contents),
            split_shebang(&with_shebang),
            "split with shebang and linefeed"
        );
        assert_eq!(
            ("", format!("{shebang}{contents}").as_str()),
            split_shebang(&with_shebang_no_lf),
            "split with shebang and no linefeed"
        );
        assert_eq!(("", contents), split_shebang(without_shebang), "split with no shebang");
    }

    fn dotfile(shell: Shell, name: &'static str, dir: &std::path::Path) -> DotfileShellIntegration {
        DotfileShellIntegration {
            shell,
            pre: true,
            post: true,
            dotfile_directory: dir.to_path_buf(),
            dotfile_name: name,
        }
    }

    /// install -> uninstall must return the rc file exactly as it was, and installing over an
    /// already-installed file must be a no-op. `install_inner` used to get this by running a full
    /// `uninstall()` (a truncating rewrite) before composing the new contents; now both paths go
    /// through `strip_from_text`, so the two have to agree.
    #[test]
    fn install_uninstall_roundtrip_is_stable() {
        let dir = tempfile::tempdir().unwrap();

        for (shell, name) in [(Shell::Zsh, ".zshrc"), (Shell::Bash, ".bashrc")] {
            let integration = dotfile(shell, name, dir.path());

            for original in [
                "# user config\nexport FOO=1\n",
                "#!/usr/bin/env zsh\n# user config\nexport FOO=1\n",
                "",
            ] {
                let installed = integration.compose(original).unwrap();
                assert!(
                    installed.contains(&integration.source_text(When::Pre).unwrap()),
                    "{name}: pre block missing"
                );
                assert!(
                    installed.contains(&integration.source_text(When::Post).unwrap()),
                    "{name}: post block missing"
                );
                // Installing again changes nothing, so `install_inner` writes nothing.
                assert_eq!(
                    integration.compose(&installed).unwrap(),
                    installed,
                    "{name}: install is not idempotent"
                );

                // Uninstalling gets back to the original, modulo the trailing-newline
                // normalisation both paths apply.
                let mut expected = original.trim().to_string();
                expected.push('\n');
                assert_eq!(
                    integration.strip_from_text(&installed).unwrap(),
                    expected,
                    "{name}: uninstall did not restore the original"
                );
            }
        }
    }

    /// `uninstall` must not rewrite a file it has nothing to remove from — it runs on rc files the
    /// integration was never installed into, and the rewrite alone rewrites trailing whitespace.
    #[tokio::test]
    async fn uninstall_leaves_an_untouched_dotfile_alone() {
        let dir = tempfile::tempdir().unwrap();
        let contents = "# user config\nexport FOO=1\n\n\n";
        let path = dir.path().join(".zshrc");
        std::fs::write(&path, contents).unwrap();

        let integration = DotfileShellIntegration {
            shell: Shell::Zsh,
            pre: false,
            post: false,
            dotfile_directory: dir.path().to_path_buf(),
            dotfile_name: ".zshrc",
        };
        integration.uninstall().await.unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }
}
