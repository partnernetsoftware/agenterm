//! Application lifecycle verbs: `app hide|show|quit|launch` and MCU aliases.

use agenterm_cu::{Command, TargetRef, command::AppAction};

use super::{flag_parsed, flag_text, flag_value, flag_window, take_switch};

/// `app <action> …`; the MCU spellings `launch PATH`, `quit`, `hide` and
/// `show` put their own name back as the action.
pub(crate) fn parse(
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spelled != "app" {
        if AppAction::parse(spelled) == Some(AppAction::Launch)
            && !args.iter().any(|arg| arg == "--path")
            && args.first().is_some_and(|first| !first.starts_with('-'))
        {
            args.insert(0, "--path".into());
        }
        args.insert(0, spelled.to_string());
    }
    let action_text = flag_value(args, "--action")
        .or_else(|| {
            args.first()
                .cloned()
                .filter(|first| !first.starts_with("--"))
        })
        .unwrap_or_default();
    if !action_text.is_empty() && args.first() == Some(&action_text) {
        args.remove(0);
    }
    let Some(action) = AppAction::parse(&action_text) else {
        return Err("app requires hide | show | quit | launch (or --action <one of them>)".into());
    };
    let window = flag_window(args)?;
    let pid = flag_parsed::<u32>(args, "--pid")?;
    // `launch` names a path rather than a running thing; `show` has no
    // window to name because hiding removed them, so the pid stands in.
    // Everything else still wants a handle.
    let launching = action == AppAction::Launch;
    if !launching && window.is_none() && pid.is_none() {
        return Err("app requires --window <handle> or --pid <n>".into());
    }
    let window = window.unwrap_or(0);
    let snapshot = take_switch(args, "--snapshot");
    let expect = flag_text(args, "--expect")?;
    let path = flag_text(args, "--path")?;
    if !args.is_empty() {
        return Err(format!(
            "app accepts only <hide|show|quit|launch> --window H | --pid N | --path P [--snapshot --expect gone]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::App {
        target,
        window,
        action,
        snapshot,
        expect,
        pid,
        path,
    })
}
