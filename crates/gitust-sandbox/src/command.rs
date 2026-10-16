//! Human-readable descriptions are never used as executable shell arguments.

pub(crate) fn display(requested: &str, required: &[String]) -> String {
    if required.is_empty() {
        return requested.to_owned();
    }
    std::iter::once(requested)
        .chain(required.iter().map(String::as_str))
        .map(|command| format!("/bin/sh -c -- '{}'", command.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" && ")
}
