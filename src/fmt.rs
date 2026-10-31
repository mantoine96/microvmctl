use std::time::Duration;

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n}B")
    } else if value >= 100.0 {
        format!("{value:.0}{}", UNITS[unit])
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

pub fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", bytes(bytes_per_sec.max(0.0) as u64))
}

pub fn duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (days, hours, mins) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60);
    match () {
        _ if days > 0 => format!("{days}d{hours}h"),
        _ if hours > 0 => format!("{hours}h{mins}m"),
        _ if mins > 0 => format!("{mins}m{}s", secs % 60),
        _ => format!("{secs}s"),
    }
}

pub fn opt<T>(value: Option<T>, f: impl FnOnce(T) -> String) -> String {
    value.map(f).unwrap_or_else(|| "-".into())
}

/// Left-aligned columns separated by two spaces, header included.
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let render = |cells: &mut dyn Iterator<Item = &str>| {
        let line: Vec<String> = cells
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        line.join("  ").trim_end().to_string()
    };
    let mut out = render(&mut header.iter().copied());
    for row in rows {
        out.push('\n');
        out.push_str(&render(&mut row.iter().map(String::as_str)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(512), "512B");
        assert_eq!(bytes(8 * 1024 * 1024 * 1024), "8.0G");
        assert_eq!(bytes(120 * 1024 * 1024 * 1024), "120G");
        assert_eq!(bytes(1536), "1.5K");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(Duration::from_secs(42)), "42s");
        assert_eq!(duration(Duration::from_secs(125)), "2m5s");
        assert_eq!(duration(Duration::from_secs(4 * 3600 + 20 * 60)), "4h20m");
        assert_eq!(duration(Duration::from_secs(3 * 86400 + 7200)), "3d2h");
    }

    #[test]
    fn aligns_table() {
        let out = table(
            &["NAME", "STATE"],
            &[
                vec!["falcon".into(), "running".into()],
                vec!["osprey".into(), "-".into()],
            ],
        );
        assert_eq!(out, "NAME    STATE\nfalcon  running\nosprey  -");
    }
}
