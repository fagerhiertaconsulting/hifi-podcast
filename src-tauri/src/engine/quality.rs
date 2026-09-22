//! Original file vs actual engine output — labels and meaningful deltas only.

/// First matching reason, in priority order: DoP vs native, resample, truncated bits.
/// Integer/float widening on the PCM f32 chain is not a delta (expected, not alarming).
pub fn quality_delta(
    format: &str,
    output_path: &str,
    resampled: bool,
    file_rate: u32,
    output_rate: u32,
    file_bits: u32,
    output_bits: u32,
) -> Vec<String> {
    let mut out = Vec::new();
    let dsd = format == "dsd";
    if dsd && output_path == "dop" {
        out.push("dop".into());
    }
    if !dsd && (resampled || rates_differ(file_rate, output_rate)) {
        out.push("resample".into());
    }
    if !dsd && bits_truncated(file_bits, output_bits) {
        out.push("bit_depth".into());
    }
    out
}

fn rates_differ(file_rate: u32, output_rate: u32) -> bool {
    file_rate > 0 && output_rate > 0 && file_rate != output_rate
}

fn bits_truncated(file_bits: u32, output_bits: u32) -> bool {
    file_bits > 0 && output_bits > 0 && output_bits < file_bits
}

pub fn quality_delta_sv(kinds: &[String]) -> String {
    kinds
        .iter()
        .filter_map(|k| match k.as_str() {
            "dop" => Some("DoP i stället för native DSD"),
            "resample" => Some("Omsampling före DAC"),
            "bit_depth" => Some("Bitdjup sänkt före DAC"),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn format_pcm_rate(hz: u32) -> String {
    if hz == 0 {
        return String::new();
    }
    if hz % 1000 == 0 {
        return format!("{} kHz", hz / 1000);
    }
    let khz = hz as f64 / 1000.0;
    if (khz * 10.0 - (khz * 10.0).round()).abs() < 1e-6 {
        let tenths = (khz * 10.0).round() as i64;
        if tenths % 10 == 0 {
            format!("{} kHz", tenths / 10)
        } else {
            format!("{}.{} kHz", tenths / 10, tenths.abs() % 10)
        }
    } else {
        format!("{hz} Hz")
    }
}

pub fn api_label_sv(api: &str) -> &'static str {
    match api {
        "asio" => "ASIO",
        "wasapi_exclusive" => "WASAPI exclusive",
        "wasapi_shared" => "WASAPI delat",
        "webview" => "WebView2",
        _ => "utgång",
    }
}

pub fn file_quality_sv(
    format: &str,
    codec: &str,
    rate: u32,
    bits: u32,
    dsd_rate: &str,
) -> String {
    if format == "dsd" {
        let rate = if dsd_rate.is_empty() {
            format_pcm_rate(rate)
        } else {
            dsd_rate.to_string()
        };
        let name = if codec.is_empty() { "DSD" } else { codec };
        if rate.is_empty() {
            name.to_string()
        } else {
            format!("{name} · {rate}")
        }
    } else {
        let mut s = if codec.is_empty() {
            "PCM".to_string()
        } else {
            codec.to_string()
        };
        let hz = format_pcm_rate(rate);
        if !hz.is_empty() {
            s.push_str(" · ");
            s.push_str(&hz);
        }
        if bits > 0 {
            s.push_str(&format!(" / {bits}-bit"));
        }
        s
    }
}

/// Short Swedish name for `output_path` (internal enum values stay English).
pub fn output_path_label_sv(output_path: &str) -> &'static str {
    match output_path {
        "native_dsd" => "Native DSD",
        "dop" => "DoP",
        "pcm" => "PCM",
        "webview" => "WebView2",
        _ => "utgång",
    }
}

pub fn output_pending_sv() -> &'static str {
    "Förhandlar ASIO/WASAPI…"
}

pub fn output_quality_sv(
    output_path: &str,
    output_api: &str,
    output_rate: u32,
    output_bits: u32,
    output_sample: &str,
    dsd_rate: &str,
) -> String {
    if output_path.is_empty() {
        return output_pending_sv().to_string();
    }
    let api = api_label_sv(output_api);
    match output_path {
        "native_dsd" => {
            let rate = if dsd_rate.is_empty() {
                format_pcm_rate(output_rate)
            } else {
                dsd_rate.to_string()
            };
            format!("{} · {rate} · {api}", output_path_label_sv(output_path))
        }
        "dop" => {
            let file = if dsd_rate.is_empty() {
                String::new()
            } else {
                format!("{dsd_rate} · ")
            };
            let pcm = format_pcm_rate(output_rate);
            let bits = if output_bits > 0 {
                format!(" / {output_bits}-bit")
            } else {
                " / 24-bit".into()
            };
            format!("DoP · {file}{pcm}{bits} · {api}")
        }
        _ => {
            let hz = format_pcm_rate(output_rate);
            let mut s = if hz.is_empty() {
                String::new()
            } else {
                hz
            };
            if output_bits > 0 {
                if !s.is_empty() {
                    s.push_str(" / ");
                }
                s.push_str(&format!("{output_bits}-bit"));
            }
            if !output_sample.is_empty() {
                if !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(output_sample);
            }
            if !s.is_empty() {
                s.push_str(" · ");
            }
            s.push_str(api);
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_dsd_same_rate_is_not_a_delta() {
        let d = quality_delta("dsd", "native_dsd", false, 11_289_600, 11_289_600, 1, 1);
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn dop_is_flagged_not_native() {
        let d = quality_delta("dsd", "dop", false, 11_289_600, 705_600, 1, 24);
        assert_eq!(d, vec!["dop".to_string()]);
        assert!(quality_delta_sv(&d).contains("DoP"));
        assert!(!quality_delta_sv(&d).contains("Omsampling"));
        assert!(!quality_delta_sv(&d).contains("Bitdjup"));
    }

    #[test]
    fn pcm_resample_is_flagged() {
        let d = quality_delta("pcm", "pcm", true, 96_000, 48_000, 24, 24);
        assert_eq!(d, vec!["resample".to_string()]);
    }

    #[test]
    fn pcm_rate_mismatch_without_flag_still_counts() {
        let d = quality_delta("pcm", "pcm", false, 96_000, 48_000, 24, 32);
        assert_eq!(d, vec!["resample".to_string()]);
    }

    #[test]
    fn pcm_f32_or_i32_widen_is_not_alarming() {
        let d = quality_delta("pcm", "pcm", false, 96_000, 96_000, 24, 32);
        assert!(d.is_empty(), "{d:?}");
        let d16 = quality_delta("pcm", "pcm", false, 44_100, 44_100, 16, 16);
        assert!(d16.is_empty(), "{d16:?}");
    }

    #[test]
    fn pcm_bit_truncation_is_flagged() {
        let d = quality_delta("pcm", "pcm", false, 96_000, 96_000, 24, 16);
        assert_eq!(d, vec!["bit_depth".to_string()]);
    }

    #[test]
    fn format_rates_are_readable() {
        assert_eq!(format_pcm_rate(96_000), "96 kHz");
        assert_eq!(format_pcm_rate(44_100), "44.1 kHz");
        assert_eq!(format_pcm_rate(705_600), "705.6 kHz");
        assert_eq!(format_pcm_rate(48_000), "48 kHz");
    }

    #[test]
    fn file_and_output_labels_pcm_and_dsd() {
        assert_eq!(
            file_quality_sv("pcm", "FLAC", 96_000, 24, ""),
            "FLAC · 96 kHz / 24-bit"
        );
        assert_eq!(
            file_quality_sv("dsd", "DSD", 11_289_600, 1, "DSD256"),
            "DSD · DSD256"
        );
        assert_eq!(
            output_quality_sv("pcm", "asio", 96_000, 32, "f32", ""),
            "96 kHz / 32-bit f32 · ASIO"
        );
        assert_eq!(
            file_quality_sv("pcm", "FLAC", 96_000, 0, ""),
            "FLAC · 96 kHz"
        );
        assert_eq!(file_quality_sv("dsd", "DSD", 0, 1, "DSD64"), "DSD · DSD64");
        assert_eq!(
            output_quality_sv("native_dsd", "asio", 11_289_600, 1, "dsd_lsb", "DSD256"),
            "Native DSD · DSD256 · ASIO"
        );
        assert_eq!(
            output_quality_sv("", "asio", 0, 0, "", ""),
            "Förhandlar ASIO/WASAPI…"
        );
        assert_eq!(output_path_label_sv("native_dsd"), "Native DSD");
        assert_eq!(
            output_quality_sv("dop", "wasapi_exclusive", 705_600, 24, "i24", "DSD256"),
            "DoP · DSD256 · 705.6 kHz / 24-bit · WASAPI exclusive"
        );
    }
}
