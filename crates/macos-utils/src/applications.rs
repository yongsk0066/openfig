use objc2_app_kit::NSWorkspace;
#[derive(Debug)]
pub struct MacOSApplication {
    pub name: Option<String>,
    pub bundle_identifier: Option<String>,
    pub bundle_path: Option<String>,
    pub process_identifier: libc::pid_t,
}

pub fn running_applications() -> Vec<MacOSApplication> {
    let workspace = NSWorkspace::sharedWorkspace();
    let apps = workspace.runningApplications();
    apps.iter()
        .map(|app| {
            let name = app.localizedName().map(|s| s.to_string());
            let bundle_identifier = app.bundleIdentifier().map(|s| s.to_string());
            let bundle_path = app.bundleURL().and_then(|url| url.path()).map(|s| s.to_string());
            let process_identifier = app.processIdentifier();

            MacOSApplication {
                name,
                bundle_identifier,
                bundle_path,
                process_identifier,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_running_applications() {
        let applications = running_applications();
        println!("{:#?}", applications);
    }
}

// launch_application("/Applications/Alacritty.app")
