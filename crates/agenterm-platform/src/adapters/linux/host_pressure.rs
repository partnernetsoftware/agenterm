use std::io::Read;

use super::contract::checked_linux_psi_window;
use super::{
    HostPressureError, HostPressureErrorKind, HostPressureSignal, HostPressureSnapshot,
    LinuxPsiWindow,
};

const PSI_FILE_BYTE_CEILING: usize = 4 * 1024;

pub(crate) fn snapshot() -> Result<HostPressureSnapshot, HostPressureError> {
    Ok(HostPressureSnapshot {
        memory: read_psi_signal("/proc/pressure/memory")?,
        cpu: read_psi_signal("/proc/pressure/cpu")?,
        io: read_psi_signal("/proc/pressure/io")?,
    })
}

fn read_psi_signal(path: &str) -> Result<HostPressureSignal, HostPressureError> {
    let file = std::fs::File::open(path).map_err(|error| {
        let kind = if error.kind() == std::io::ErrorKind::NotFound {
            HostPressureErrorKind::ProviderUnavailable
        } else {
            HostPressureErrorKind::NativeQuery
        };
        HostPressureError::new(kind, format!("open {path}: {error}"))
    })?;
    let mut bytes = Vec::with_capacity(PSI_FILE_BYTE_CEILING.min(512));
    file.take((PSI_FILE_BYTE_CEILING + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            HostPressureError::new(
                HostPressureErrorKind::NativeQuery,
                format!("read {path}: {error}"),
            )
        })?;
    if bytes.len() > PSI_FILE_BYTE_CEILING {
        return Err(HostPressureError::new(
            HostPressureErrorKind::MalformedNativeData,
            format!("{path} exceeds {PSI_FILE_BYTE_CEILING} bytes"),
        ));
    }
    let contents = std::str::from_utf8(&bytes).map_err(|_| {
        HostPressureError::new(
            HostPressureErrorKind::MalformedNativeData,
            format!("{path} is not UTF-8"),
        )
    })?;
    let (some, full) = parse_psi(contents)?;
    Ok(HostPressureSignal::LinuxPsi { some, full })
}

fn parse_psi(
    contents: &str,
) -> Result<(LinuxPsiWindow, Option<LinuxPsiWindow>), HostPressureError> {
    let mut some = None;
    let mut full = None;

    for line in contents.lines() {
        let mut fields = line.split_ascii_whitespace();
        let class = fields
            .next()
            .ok_or_else(|| malformed("empty Linux PSI line"))?;
        let window = parse_psi_window(fields)?;
        let destination = match class {
            "some" => &mut some,
            "full" => &mut full,
            _ => return Err(malformed(format!("unknown Linux PSI class: {class}"))),
        };
        if destination.replace(window).is_some() {
            return Err(malformed(format!("duplicate Linux PSI class: {class}")));
        }
    }

    let some = some.ok_or_else(|| malformed("Linux PSI data has no some row"))?;
    Ok((some, full))
}

fn parse_psi_window<'a>(
    fields: impl Iterator<Item = &'a str>,
) -> Result<LinuxPsiWindow, HostPressureError> {
    let mut averages = [None; 3];
    let mut total = None;

    for field in fields {
        let (name, value) = field
            .split_once('=')
            .ok_or_else(|| malformed(format!("Linux PSI field has no value: {field}")))?;
        match name {
            "avg10" => parse_average(&mut averages[0], name, value)?,
            "avg60" => parse_average(&mut averages[1], name, value)?,
            "avg300" => parse_average(&mut averages[2], name, value)?,
            "total" => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| invalid_value(name, value))?;
                if total.replace(parsed).is_some() {
                    return Err(malformed("duplicate Linux PSI field: total"));
                }
            }
            _ => return Err(malformed(format!("unknown Linux PSI field: {name}"))),
        }
    }

    let averages = [
        required(averages[0], "avg10")?,
        required(averages[1], "avg60")?,
        required(averages[2], "avg300")?,
    ];
    checked_linux_psi_window(averages, required(total, "total")?)
}

fn parse_average(
    destination: &mut Option<f64>,
    name: &str,
    value: &str,
) -> Result<(), HostPressureError> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| invalid_value(name, value))?;
    if destination.replace(parsed).is_some() {
        return Err(malformed(format!("duplicate Linux PSI field: {name}")));
    }
    Ok(())
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, HostPressureError> {
    value.ok_or_else(|| malformed(format!("Linux PSI row has no {name}")))
}

fn malformed(detail: impl Into<String>) -> HostPressureError {
    HostPressureError::new(HostPressureErrorKind::MalformedNativeData, detail)
}

fn invalid_value(name: &str, value: &str) -> HostPressureError {
    HostPressureError::new(
        HostPressureErrorKind::InvalidNativeValue,
        format!("invalid Linux PSI {name} value: {value}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_required_some_and_optional_full_rows() {
        let (some, full) = parse_psi(
            "some avg10=1.25 avg60=2.50 avg300=3.75 total=42\n\
             full avg10=0.01 avg60=0.02 avg300=0.03 total=7\n",
        )
        .unwrap();
        assert_eq!(some.average_10_seconds, 1.25);
        assert_eq!(some.average_60_seconds, 2.5);
        assert_eq!(some.average_300_seconds, 3.75);
        assert_eq!(some.total_stall_microseconds, 42);
        assert_eq!(full.unwrap().total_stall_microseconds, 7);

        let (_, full) = parse_psi("some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n").unwrap();
        assert_eq!(full, None);
    }

    #[test]
    fn parser_rejects_missing_or_duplicate_structure() {
        for input in [
            "full avg10=0 avg60=0 avg300=0 total=0\n",
            "some avg10=0 avg60=0 total=0\n",
            "some avg10=0 avg60=0 avg300=0 total=0 total=1\n",
            "some avg10=0 avg60=0 avg300=0 total=0\n\
             some avg10=0 avg60=0 avg300=0 total=1\n",
            "some avg10=0 avg60=0 avg300=0 total=0 future=1\n",
        ] {
            assert_eq!(
                parse_psi(input).unwrap_err().kind(),
                HostPressureErrorKind::MalformedNativeData
            );
        }
    }

    #[test]
    fn parser_rejects_invalid_numeric_values() {
        for input in [
            "some avg10=nope avg60=0 avg300=0 total=0\n",
            "some avg10=-0.01 avg60=0 avg300=0 total=0\n",
            "some avg10=100.01 avg60=0 avg300=0 total=0\n",
            "some avg10=0 avg60=0 avg300=0 total=nope\n",
        ] {
            assert_eq!(
                parse_psi(input).unwrap_err().kind(),
                HostPressureErrorKind::InvalidNativeValue
            );
        }
    }
}
