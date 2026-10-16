//! Keep shell arguments unexpanded while retaining a readable execution description.

pub(crate) struct ShellCommand {
    pub(crate) script: String,
    pub(crate) args: Vec<String>,
    pub(crate) display: String,
}

impl ShellCommand {
    pub(crate) fn new(requested: &str, required: &[String]) -> Self {
        if required.is_empty() {
            return Self {
                script: requested.to_owned(),
                args: Vec::new(),
                display: requested.to_owned(),
            };
        }
        let commands = std::iter::once(requested).chain(required.iter().map(String::as_str));
        let display = commands
            .clone()
            .map(|command| format!("/bin/sh -c '{}'", command.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" && ");
        // Pass each original command as an argument: shell escaping can quadruple its
        // size and exceed Linux's per-argument limit even for an accepted request.
        let args = std::iter::once("gitust-checks")
            .chain(commands)
            .map(str::to_owned)
            .collect();
        Self {
            script: "for command do /bin/sh -c \"$command\" || exit $?; done".into(),
            args,
            display,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_maximum_request_stays_below_linux_per_argument_limit() {
        let requested = format!("true {}", "''".repeat(32_765));
        assert_eq!(requested.len(), 65_535);
        let command = ShellCommand::new(&requested, &["true".into()]);
        assert!(command.display.len() > 131_072);
        assert!(command.script.len() < 131_072);
        assert!(command.args.iter().all(|arg| arg.len() < 131_072));
        assert_eq!(command.args, ["gitust-checks", &requested, "true"]);
    }
}
