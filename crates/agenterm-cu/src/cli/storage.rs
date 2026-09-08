//! Mounted-volume inventory argv parsing.

use agenterm_cu::{Command, TargetRef, command::STORAGE_VOLUMES_MAX};

use super::{flag_parsed, verbs::VerbSpec};

pub fn parse(
    spec: &VerbSpec,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    debug_assert_eq!(spec.name, "storage-volumes");
    if args.first().is_some_and(|arg| arg == "volumes") {
        args.remove(0);
    }
    let max = flag_parsed::<usize>(args, "--max")?.unwrap_or(128);
    if !(1..=STORAGE_VOLUMES_MAX).contains(&max) {
        return Err("storage volumes --max must be in 1..=512".to_owned());
    }
    if !args.is_empty() {
        return Err(format!(
            "storage-volumes accepts only --max N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::StorageVolumes { target, max })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> &'static VerbSpec {
        crate::cli::verbs::lookup("storage-volumes").expect("catalog storage-volumes")
    }

    #[test]
    fn flat_and_grouped_shapes_are_closed() {
        assert!(matches!(
            parse(
                spec(),
                TargetRef::Current,
                &mut vec!["--max".into(), "7".into()]
            )
            .unwrap(),
            Command::StorageVolumes { max: 7, .. }
        ));
        assert!(matches!(
            parse(spec(), TargetRef::Current, &mut vec!["volumes".into()]).unwrap(),
            Command::StorageVolumes { max: 128, .. }
        ));
        for max in ["0", "513"] {
            assert!(
                parse(
                    spec(),
                    TargetRef::Current,
                    &mut vec!["--max".into(), max.into()]
                )
                .is_err()
            );
        }
    }
}
