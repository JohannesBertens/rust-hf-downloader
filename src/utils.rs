pub fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn format_size(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_below_kb_boundary() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn kb_boundary_switches_to_kb() {
        assert_eq!(format_size(1024), "1.00 KB");
    }

    #[test]
    fn mb_and_gb_boundaries() {
        assert_eq!(format_size(1_048_576), "1.00 MB");
        assert_eq!(format_size(1_073_741_824), "1.00 GB");
        assert_eq!(format_size(5_368_709_120), "5.00 GB");
    }

    #[test]
    fn format_number_abbreviates() {
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1.0K");
        assert_eq!(format_number(1_234_567), "1.2M");
    }
}
