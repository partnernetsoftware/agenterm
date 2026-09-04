//! Host-level discovery that is independent of the desktop/window family.

use agenterm_cu::{Command, TargetRef};

use super::verbs::VerbSpec;

pub fn parse(spec: &VerbSpec, target: TargetRef, args: &mut [String]) -> Result<Command, String> {
    if !args.is_empty() {
        return Err(format!(
            "{} accepts no arguments; unexpected {:?}",
            spec.name, args[0]
        ));
    }
    match spec.name {
        "capabilities" => Ok(Command::Capabilities { target }),
        "permissions" => Ok(Command::Permissions { target }),
        other => Err(format!("unknown command '{other}'")),
    }
}
