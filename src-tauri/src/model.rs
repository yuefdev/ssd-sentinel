use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Health {
    pub percent: Option<u32>,
    pub status: String,
    pub method: Option<String>,
    pub explanation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SmartRow {
    pub id: String,
    pub name: String,
    pub original: String,
    pub raw: String,
    pub value: Option<f64>,
    pub worst: Option<f64>,
    pub threshold: Option<f64>,
    pub description: String,
    pub level: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alert {
    pub code: String,
    pub level: String,
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Disk {
    pub id: String,
    pub stable_identity: bool,
    pub device: String,
    pub device_type: String,
    pub model: String,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub protocol: Option<String>,
    pub metrics: Value,
    pub health: Health,
    pub smart: Vec<SmartRow>,
    pub alerts: Vec<Alert>,
    pub capabilities: Value,
    pub interface: Option<String>,
    pub logical_block_size: Option<u64>,
    pub physical_block_size: Option<u64>,
    pub rotation_rate: Option<u64>,
    pub nvme_eui64: Vec<String>,
    pub source_time: Option<i64>,
    pub os_disk_number: Option<u64>,
    pub os_serial: Option<String>,
    pub os_model: Option<String>,
    pub volumes: Vec<Value>,
    #[serde(default)]
    pub bus_link: Value,
    #[serde(default)]
    pub optimization: Value,
    #[serde(default)]
    pub read_incomplete: bool,
    #[serde(default)]
    pub read_warning: Option<String>,
}

pub fn unsigned(value: &Value) -> Option<String> {
    if !value.is_number() {
        return None;
    }
    let text = value.to_string();
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())).then_some(text)
}

fn metric(value: &Value) -> Value {
    if value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0) {
        value.clone()
    } else {
        Value::Null
    }
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

fn format_nvme_interface(raw: String) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return raw;
    }
    if trimmed.to_ascii_lowercase().starts_with("nvme") {
        trimmed.to_owned()
    } else if trimmed.bytes().next().is_some_and(|b| b.is_ascii_digit()) {
        format!("NVMe {trimmed}")
    } else {
        trimmed.to_owned()
    }
}

pub fn health(s: &Value) -> Health {
    health_with_mode(s, false)
}

fn find_unique_attr(rows: &[Value], id: u64) -> Option<&Value> {
    let mut iter = rows.iter().filter(|r| r["id"] == id);
    let first = iter.next()?;
    if iter.next().is_some() {
        return None;
    }
    Some(first)
}

fn low16_raw(row: &Value) -> Option<u32> {
    let raw = unsigned(&row["raw"]["value"])?;
    Some(
        raw.bytes()
            .fold(0u32, |n, b| (n * 10 + u32::from(b - b'0')) & 0xffff),
    )
}

fn sata_ssd_wear_indicator(rows: &[Value]) -> Option<(u64, &str, u32)> {
    let check_name = |row: &Value, needles: &[&str]| {
        let name = row["name"].as_str().unwrap_or("").to_ascii_lowercase();
        needles.iter().any(|k| name.contains(k))
    };
    for (id, needles) in [
        (177u64, &[][..]),
        (233u64, &[][..]),
        (202u64, &["life", "percent", "wear"][..]),
        (231u64, &["life", "wear", "remain"][..]),
        (169u64, &["life", "wear", "erase"][..]),
        (173u64, &["life", "wear", "erase"][..]),
    ] {
        if let Some(row) = find_unique_attr(rows, id) {
            if !needles.is_empty() && !check_name(row, needles) {
                continue;
            }
            if let Some(val) = row["value"].as_u64().filter(|v| *v <= 100) {
                let name = row["name"].as_str().unwrap_or("SMART");
                return Some((id, name, val as u32));
            }
        }
    }
    None
}

pub fn health_with_mode(s: &Value, strict: bool) -> Health {
    let mut h = Health { percent: None, status: "unknown".into(), method: None,
        explanation: "Gerekli alanlar eksik veya model kuralı desteklenmiyor. Bilinmeyen veri sağlıklı sayılmaz.".into() };
    if s.get("smartctl").is_some_and(|v| !v.is_object()) {
        return h;
    }
    let exit = s
        .pointer("/smartctl/exit_status")
        .cloned()
        .unwrap_or(json!(0));
    let Some(exit) = exit.as_u64().filter(|x| *x <= 255) else {
        return h;
    };
    if exit & 7 != 0 {
        h.explanation =
            "SMART okuması eksik. Yönetici erişimini ve aygıt bağlantısını kontrol edin.".into();
        return h;
    }
    if s.pointer("/smart_status/passed") == Some(&json!(false)) {
        h.status = "critical".into();
        return h;
    }
    if s.pointer("/device/protocol").and_then(Value::as_str) == Some("NVMe") {
        h.method = Some("nvme-remaining-endurance".into());
        let log = &s["nvme_smart_health_information_log"];
        let Some(critical) = log["critical_warning"].as_u64().filter(|v| *v <= 255) else {
            return h;
        };
        if critical != 0 {
            h.status = "critical".into();
            h.explanation =
                format!("NVMe kritik uyarı maskesi: {critical}. Önce yedeğinizi doğrulayın.");
            return h;
        }
        let (Some(spare), Some(threshold)) = (
            log["available_spare"].as_u64().filter(|x| *x <= 100),
            log["available_spare_threshold"]
                .as_u64()
                .filter(|x| *x <= 100),
        ) else {
            return h;
        };
        if spare < threshold {
            h.status = "critical".into();
            h.explanation = "Kullanılabilir yedek alan üretici eşiğinin altında.".into();
            return h;
        }
        let Some(used) = log["percentage_used"].as_u64().filter(|v| *v <= 255) else {
            return h;
        };
        if unsigned(&log["media_errors"]).is_none() {
            return h;
        }
        h.percent = Some(100u64.saturating_sub(used) as u32);
        h.status = if unsigned(&log["media_errors"]).is_some_and(|v| v != "0")
            || unsigned(&log["num_err_log_entries"]).is_some_and(|v| v != "0")
        {
            "warning"
        } else {
            "ok"
        }
        .into();
        h.explanation = format!("Üretici tüketilen dayanımı %{used} bildiriyor. Kalan dayanım = max(0, 100 − {used}). Bu bir hız ölçümü veya kesin arıza tarihi değildir.");
    } else if s.pointer("/device/protocol").and_then(Value::as_str) == Some("ATA")
        && s["rotation_rate"].as_u64().is_some_and(|r| r > 0)
    {
        h.method = Some(
            if strict {
                "hdd-strict-estimate"
            } else {
                "hdd-generic-core-estimate"
            }
            .into(),
        );
        if s.pointer("/smart_status/passed") != Some(&json!(true)) {
            return h;
        }
        let Some(rows) = s
            .pointer("/ata_smart_attributes/table")
            .and_then(Value::as_array)
        else {
            return h;
        };
        if rows.iter().any(|r| !r.is_object()) {
            return h;
        }
        if rows.iter().any(|r| r["when_failed"] == "now") {
            h.status = "critical".into();
            return h;
        }
        let coeffs = if strict {
            [
                (5, 60, 720),
                (7, 20, 200),
                (10, 60, 660),
                (196, 40, 360),
                (197, 40, 520),
                (198, 60, 720),
            ]
        } else {
            [
                (5, 10, 700),
                (7, 5, 200),
                (10, 30, 600),
                (196, 6, 300),
                (197, 6, 480),
                (198, 10, 700),
            ]
        };
        let mut numerator = 100u128;
        let mut s_count: u64 = 0;
        for (id, weight, cap) in coeffs {
            let Some(row) = find_unique_attr(rows, id) else {
                return h;
            };
            let Some(low) = low16_raw(row) else {
                return h;
            };
            if id == 5 || id == 198 || (strict && id == 197) {
                s_count = s_count.saturating_add(u64::from(low));
            }
            numerator *= u128::from(1000 - (low * weight).min(cap));
        }
        let denominator = 1000u128.pow(6);
        let rounded = if !strict && s_count <= 70 {
            let whole = numerator / denominator;
            let remainder = numerator % denominator;
            (whole
                + u128::from(
                    remainder * 2 > denominator || (remainder * 2 == denominator && whole % 2 == 1),
                )) as u32
        } else {
            let mut p = (numerator as f64) / (denominator as f64);
            if strict {
                if (11..=99).contains(&s_count) {
                    p *= 0.99_f64.powf(((s_count - 10) as f64) * 5.0 / 9.0);
                } else if (100..=300).contains(&s_count) {
                    p *= 0.99_f64.powf(s_count as f64 / 2.0);
                } else if s_count > 300 {
                    p = p.min(9.0);
                }
            } else if (71..=99).contains(&s_count) {
                p *= 0.99_f64.powf((s_count - 70) as f64 / 3.0);
            } else if (100..=1200).contains(&s_count) {
                p *= 0.99_f64.powf(s_count as f64 / 10.0);
            } else if s_count > 1200 {
                p = p.min(9.0);
            }
            p.clamp(0.0, 100.0).round_ties_even() as u32
        };
        h.percent = Some(rounded);
        h.status = if rounded < 100 { "warning" } else { "ok" }.into();
        h.explanation = if strict {
            "Sıkı mod: 05, 07, 0A, C4, C5 ve C6 alanlarına yüksek ağırlık ve iç sayaç kuyruk cezası uygulandı.".into()
        } else {
            "05, 07, 0A, C4, C5 ve C6 alanlarının genel ceza çarpımı. Model istisnaları ve ek tavanlar tamamlanmadığından çekirdek tahminidir.".into()
        };
    } else if s.pointer("/device/protocol").and_then(Value::as_str) == Some("ATA")
        && s["rotation_rate"] == 0
    {
        if s.pointer("/smart_status/passed") != Some(&json!(true)) {
            return h;
        }
        let Some(rows) = s
            .pointer("/ata_smart_attributes/table")
            .and_then(Value::as_array)
        else {
            return h;
        };
        if rows.iter().any(|r| !r.is_object()) {
            return h;
        }
        if rows.iter().any(|r| r["when_failed"] == "now") {
            h.status = "critical".into();
            return h;
        }
        if let Some((attr_id, attr_name, base)) = sata_ssd_wear_indicator(rows) {
            let has_err = [5u64, 184, 187, 197, 198].iter().any(|id| {
                find_unique_attr(rows, *id)
                    .and_then(|r| unsigned(&r["raw"]["value"]))
                    .is_some_and(|v| v != "0")
            });
            let mut percent = base;
            if strict {
                let mut p = f64::from(base);
                for (id, w, cap) in [
                    (5u64, 6u32, 72u32),
                    (187, 4, 50),
                    (197, 4, 52),
                    (198, 6, 72),
                ] {
                    if let Some(low) = find_unique_attr(rows, id).and_then(low16_raw) {
                        if low > 0 {
                            let penalty = f64::from((low * w).min(cap));
                            p *= (100.0 - penalty) / 100.0;
                        }
                    }
                }
                percent = p.clamp(0.0, 100.0).round_ties_even() as u32;
            }
            h.method = Some(
                if strict {
                    "sata-ssd-strict-wear"
                } else {
                    "sata-ssd-wear-indicator"
                }
                .into(),
            );
            h.percent = Some(percent);
            h.status = if has_err || (strict && percent < base) {
                "warning"
            } else {
                "ok"
            }
            .into();
            h.explanation = format!(
                "SATA SSD üretici aşınma niteliği #{attr_id} ({attr_name}) üzerinden kalan dayanım %{percent}{} olarak hesaplandı.",
                if strict { " (sıkı hata cezası dahil)" } else { "" }
            );
        }
    }
    if h.percent == Some(0) || (exit > 0 && h.percent.is_some()) {
        h.status = "warning".into();
    }
    h
}

fn ata_field(id: u64) -> (&'static str, &'static str) {
    match id {
        1 => (
            "Okuma hata oranı",
            "Üreticiye özgü oran/sayaç. Ham sayı tek başına bozuk sektör sayısı değildir.",
        ),
        3 => (
            "Motorun başlama süresi",
            "HDD motorunun çalışma hızına ulaşma süresi. Birim üreticiye bağlıdır.",
        ),
        4 => (
            "Başlatma / durdurma",
            "Disk motorunun başlatılıp durdurulma sayısı.",
        ),
        5 => (
            "Yeniden eşlenen sektör",
            "Yedek alanla değiştirilen sektörler. Artış takip edilmelidir.",
        ),
        7 => (
            "Konumlandırma hata oranı",
            "Kafa konumlandırma ölçümü; ham kodlama üreticiye özgü olabilir.",
        ),
        9 => (
            "Çalışma süresi",
            "Ham zaman birimi modele bağlıdır; özet smartctl tarafından çözülen saati kullanır.",
        ),
        10 => (
            "Motor başlatma tekrarı",
            "İlk denemede başlayamayan motor olayları; güç beslemesiyle ilişkili olabilir.",
        ),
        12 => ("Açılma sayısı", "Aygıtın güç döngüsü sayısı."),
        184 => (
            "Uçtan uca veri hatası",
            "Veri yolundaki bütünlük kontrolü hataları.",
        ),
        187 => (
            "Düzeltilemeyen hata",
            "Ana sisteme bildirilen düzeltilemeyen veri/komut hataları.",
        ),
        188 => (
            "Komut zaman aşımı",
            "Tamamlanamayan komutlar; ham alan birden fazla sayaç içerebilir.",
        ),
        190 | 194 => (
            "Disk sıcaklığı",
            "Üretici sıcaklık alanı; geçmiş min/max değerlerini de içerebilir.",
        ),
        191 => ("Sarsıntı olayı", "Algılanan mekanik şok ve titreşimler."),
        192 => (
            "Güç kesilmesinde park",
            "Beklenmeyen kapanmada park olayları; SSD anlamı modele bağlıdır.",
        ),
        193 => (
            "Yükleme / park döngüsü",
            "HDD kafalarının yüklenip park edilme sayısı.",
        ),
        196 => (
            "Yeniden eşleme olayı",
            "Yeniden eşleme denemelerinin sayısı; sektör sayısıyla aynı olmayabilir.",
        ),
        197 => (
            "Bekleyen sektör",
            "Kararsız okunabilen sektörler. Önce önemli verilerin yedeğini doğrulayın.",
        ),
        198 => (
            "Düzeltilemeyen sektör",
            "Çevrimdışı kontrolde düzeltilemeyen alanlar; veri bütünlüğü riski.",
        ),
        199 => (
            "Bağlantı CRC hatası",
            "SATA kablosu/veri yolu aktarım hataları. Disk yüzeyi hatası değildir.",
        ),
        169 => (
            "Kalan ömür yüzdesi",
            "Üretici SATA SSD kalan ömür göstergesi.",
        ),
        173 => (
            "Aşınma dengeleme sayacı",
            "NAND bloklarının ortalama/maksimum silme döngüsü ve kalan ömür yüzdesi.",
        ),
        177 => (
            "Aşınma dengeleme (Samsung)",
            "NAND silme döngüsü ve kalan blok dayanımı.",
        ),
        202 | 231 | 233 => (
            "Üretici ömür göstergesi",
            "SSD kalan dayanım yüzdesi veya üretici ömür alanı; adı sıcaklık olan 231 alanları ömür sayılmaz.",
        ),
        241 => (
            "Toplam yazma sayacı",
            "Birim üreticiye bağlıdır; model kanıtı olmadan TB dönüşümü yapılmaz.",
        ),
        242 => (
            "Toplam okuma sayacı",
            "Birim üreticiye bağlıdır; model kanıtı olmadan TB dönüşümü yapılmaz.",
        ),
        _ => (
            "",
            "Üreticiye özgü SMART alanı; model doğrulaması olmadan sağlık cezası çıkarılmaz.",
        ),
    }
}

fn nvme_field(key: &str) -> (&str, &str) {
    match key {
        "critical_warning" => ("Kritik uyarılar", "Bit maskesi: yedek alan, sıcaklık, güvenilirlik, salt okunur durum ve kalıcı bellek."),
        "temperature" => ("Bileşik sıcaklık", "Denetleyicinin bildirdiği ana sıcaklık, °C."),
        "available_spare" => ("Kullanılabilir yedek alan", "Kalan yedek kapasite yüzdesi; eşikle karşılaştırılır."),
        "available_spare_threshold" => ("Yedek alan eşiği", "Yedek kapasite bu yüzdenin altına düşerse kritik durum oluşur."),
        "percentage_used" => ("Tüketilen dayanım", "Üretici tahmini; %100'ü aşabilir. Kesin arıza tarihi değildir."),
        "data_units_read" => ("Okunan veri birimi", "Ömür boyu host okuması. Bir NVMe birimi 512.000 byte."),
        "data_units_written" => ("Yazılan veri birimi", "Ömür boyu host yazması; fiziksel NAND yazması değildir. Birim 512.000 byte."),
        "host_reads" => ("Okuma komutu", "Tamamlanan ana sistem okuma komutları."),
        "host_writes" => ("Yazma komutu", "Tamamlanan ana sistem yazma komutları."),
        "controller_busy_time" => ("Denetleyici meşgul süresi", "I/O komutlarıyla meşgul geçen toplam dakika."),
        "power_cycles" => ("Güç döngüsü", "Aygıtın açılma sayısı."),
        "power_on_hours" => ("Çalışma süresi", "Güç açık geçen toplam saat."),
        "unsafe_shutdowns" => ("Güvensiz kapanma", "Düzenli kapanma bildirimi alınmadan güç kesilmesi. Tek başına veri kaybı kanıtı değil."),
        "media_errors" => ("Medya / veri bütünlüğü hatası", "Düzeltilemeyen veri bütünlüğü olayları; artış varsa yedek ve inceleme öncelikli."),
        "num_err_log_entries" => ("Hata günlüğü kaydı", "Geçersiz/desteklenmeyen komutlar da sayılır. Bozuk sektör sayısı değildir."),
        "warning_temp_time" => ("Sıcaklık uyarısında süre", "Uyarı sıcaklığı üzerinde geçen toplam dakika."),
        "critical_comp_time" => ("Kritik sıcaklıkta süre", "Kritik sıcaklık üzerinde geçen toplam dakika."),
        _ => (key, "NVMe/üretici alanı; bu sürümde özel yorumu bulunmuyor."),
    }
}

fn sata_stat_sectors(s: &Value, target: &str) -> Option<String> {
    let pages = s.pointer("/ata_device_statistics/pages")?.as_array()?;
    for page in pages {
        if let Some(table) = page["table"].as_array() {
            for row in table {
                if row["name"].as_str() == Some(target) {
                    if let Some(n) = unsigned(&row["value"]).and_then(|v| v.parse::<u128>().ok()) {
                        return n.checked_mul(512).map(|v| v.to_string());
                    }
                }
            }
        }
    }
    None
}

fn sata_attr_bytes(s: &Value, id: u64) -> Option<String> {
    let rows = s.pointer("/ata_smart_attributes/table")?.as_array()?;
    let row = find_unique_attr(rows, id)?;
    let raw = unsigned(&row["raw"]["value"])?.parse::<u128>().ok()?;
    let name = row["name"].as_str().unwrap_or("").to_ascii_lowercase();
    let unit = if name.contains("lba") {
        512
    } else if name.contains("32mib") {
        33_554_432
    } else if name.contains("gib") {
        1_073_741_824
    } else {
        return None;
    };
    raw.checked_mul(unit).map(|v| v.to_string())
}

pub fn normalise(s: &Value) -> Disk {
    normalise_with_mode(s, false)
}

pub fn normalise_with_mode(s: &Value, strict: bool) -> Disk {
    let model = text(&s["model_name"]);
    let serial = text(&s["serial_number"]);
    let stable = model.as_ref().is_some_and(|m| !m.trim().is_empty())
        && serial.as_ref().is_some_and(|m| !m.trim().is_empty());
    let identity = if stable {
        format!("[{}, {}]", json!(model), json!(serial))
    } else {
        format!("[\"unknown\", {}]", s["device"]["name"])
    };
    let id = format!("{:x}", Sha256::digest(identity.as_bytes()))[..24].to_owned();
    let protocol = text(&s["device"]["protocol"]);
    let kind = if protocol.as_deref() == Some("NVMe") {
        "NVMe SSD"
    } else if s["rotation_rate"] == 0 {
        "SATA SSD"
    } else if s["rotation_rate"].as_u64().is_some_and(|r| r > 0) {
        "HDD"
    } else {
        "Bilinmiyor"
    };
    let h = health_with_mode(s, strict);
    let log = &s["nvme_smart_health_information_log"];
    let choose = |a: &Value, b: &Value| metric(if a.is_null() { b } else { a });
    let bytes = |v: &Value| {
        unsigned(v)
            .and_then(|v| v.parse::<u128>().ok())
            .and_then(|v| v.checked_mul(512000))
            .map(|v| v.to_string())
    };
    let written_bytes = bytes(&log["data_units_written"]).or_else(|| {
        (s["rotation_rate"] == 0).then(|| {
            sata_stat_sectors(s, "Logical Sectors Written").or_else(|| sata_attr_bytes(s, 241))
        })?
    });
    let read_bytes = bytes(&log["data_units_read"]).or_else(|| {
        (s["rotation_rate"] == 0).then(|| {
            sata_stat_sectors(s, "Logical Sectors Read").or_else(|| sata_attr_bytes(s, 242))
        })?
    });
    let metrics = json!({
        "temperature_c": choose(&s["temperature"]["current"], &log["temperature"]),
        "temperature_limit_c": metric(&s["temperature"]["op_limit_max"]),
        "capacity_bytes": choose(&s["user_capacity"]["bytes"], &s["nvme_total_capacity"]),
        "power_on_hours": choose(&s["power_on_time"]["hours"], &log["power_on_hours"]),
        "power_cycles": choose(&s["power_cycle_count"], &log["power_cycles"]),
        "written_bytes": written_bytes, "read_bytes": read_bytes,
        "percentage_used": metric(&log["percentage_used"]), "media_errors": metric(&log["media_errors"]),
        "unsafe_shutdowns": metric(&log["unsafe_shutdowns"])
    });
    let mut alerts = Vec::new();
    let mut add = |code: &str, level: &str, title: &str, detail: &str| {
        alerts.push(Alert {
            code: code.into(),
            level: level.into(),
            title: title.into(),
            detail: detail.into(),
        })
    };
    if h.status == "critical" {
        add(
            "critical",
            "critical",
            "Aygıt kritik durum bildiriyor",
            "Önemli verilerin yedeğini doğrulayın ve üretici tanısını inceleyin.",
        );
    }
    if h.percent.is_none() {
        add(
            "unknown-health",
            "warning",
            "Sağlık puanı doğrulanamadı",
            &h.explanation,
        );
    }
    if h.percent.is_some_and(|v| v <= 20) {
        add(
            "low-health",
            "critical",
            "Sağlık / dayanım düşük",
            "Güncel yedeği doğrulayın ve disk değişimini planlayın.",
        );
    }
    if metrics["temperature_c"]
        .as_f64()
        .is_some_and(|v| v >= metrics["temperature_limit_c"].as_f64().unwrap_or(65.0))
    {
        add(
            "temperature",
            "warning",
            "Disk sıcaklığı yüksek",
            "Soğutmayı, hava akışını ve iş yükünü kontrol edin.",
        );
    }
    let mut bus_link = Value::Null;
    if protocol.as_deref() != Some("NVMe") {
        if let (Some(cur_ups), Some(max_ups)) = (
            s.pointer("/interface_speed/current/units_per_second")
                .and_then(Value::as_u64),
            s.pointer("/interface_speed/max/units_per_second")
                .and_then(Value::as_u64),
        ) {
            let cur_str = s
                .pointer("/interface_speed/current/string")
                .and_then(Value::as_str)
                .unwrap_or("");
            let max_str = s
                .pointer("/interface_speed/max/string")
                .and_then(Value::as_str)
                .unwrap_or("");
            let bottleneck = cur_ups < max_ups;
            bus_link = json!({
                "kind": "SATA",
                "current": cur_str,
                "max": max_str,
                "current_units_per_sec": cur_ups,
                "max_units_per_sec": max_ups,
                "bottleneck": bottleneck
            });
            if bottleneck {
                add(
                    "sata-link-bottleneck",
                    "warning",
                    "SATA bağlantı hızı düşük",
                    &format!(
                        "Aygıt şu an {cur_str} hızında çalışıyor; desteklenen en yüksek hız {max_str}. Kablo veya SATA portunu kontrol edin."
                    ),
                );
            }
        }
    }
    let mut smart = Vec::new();
    if let Some(log) = log.as_object().filter(|m| !m.is_empty()) {
        for (key, val) in log {
            if key == "nsid" || val.is_array() || val.is_object() {
                continue;
            }
            let (name, description) = nvme_field(key);
            let nonzero = unsigned(val).is_some_and(|v| v != "0");
            let level = if key == "critical_warning" && nonzero {
                "critical"
            } else if key == "media_errors" && nonzero {
                "warning"
            } else if matches!(key.as_str(), "num_err_log_entries" | "unsafe_shutdowns") && nonzero
            {
                "info"
            } else {
                "ok"
            };
            let raw = val
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| val.to_string());
            if matches!(key.as_str(), "media_errors" | "num_err_log_entries") && nonzero {
                add(key, level, &format!("{raw} · {name}"), description);
            }
            smart.push(SmartRow {
                id: key.clone(),
                name: name.into(),
                original: key.clone(),
                raw,
                value: None,
                worst: None,
                threshold: None,
                description: description.into(),
                level: level.into(),
            });
        }
    } else if let Some(rows) = s
        .pointer("/ata_smart_attributes/table")
        .and_then(Value::as_array)
    {
        for row in rows.iter().filter(|r| r.is_object()) {
            let id = row["id"].as_u64().unwrap_or(0);
            let (title, description) = ata_field(id);
            let original = row["name"].as_str().unwrap_or("SMART alanı");
            let name = if title.is_empty() { original } else { title };
            let raw = unsigned(&row["raw"]["value"])
                .or_else(|| text(&row["raw"]["string"]))
                .unwrap_or_else(|| "Bilinmiyor".into());
            let nonzero = unsigned(&row["raw"]["value"]).is_some_and(|v| v != "0");
            let level = if row["when_failed"] == "now" {
                "critical"
            } else if matches!(id, 5 | 10 | 184 | 187 | 196 | 197 | 198) && nonzero {
                "warning"
            } else if id == 199 && nonzero {
                "info"
            } else {
                "ok"
            };
            if level != "ok" {
                add(
                    &format!("ata-{id}"),
                    level,
                    name,
                    &format!("Ham değer: {raw}. {description}"),
                );
            }
            smart.push(SmartRow {
                id: id.to_string(),
                name: name.into(),
                original: original.into(),
                raw,
                value: row["value"].as_f64(),
                worst: row["worst"].as_f64(),
                threshold: row["thresh"].as_f64(),
                description: description.into(),
                level: level.into(),
            });
        }
    }
    let nvme_eui64 = s["nvme_namespaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let oui = n["eui64"]["oui"].as_u64()?;
            let ext = n["eui64"]["ext_id"].as_u64()?;
            (oui < (1 << 24) && ext < (1 << 40)).then(|| format!("{oui:06x}{ext:010x}"))
        })
        .collect();
    let exit = s["smartctl"]["exit_status"].as_u64();
    let read_incomplete = exit.is_some_and(|code| code & 7 != 0);
    let dev_name = s["device"]["name"].as_str().unwrap_or("?");
    let read_warning = if read_incomplete {
        Some(format!(
            "{dev_name}: SMART okuması eksik (kod {}).",
            exit.unwrap_or(0)
        ))
    } else {
        None
    };
    Disk {
        id,
        stable_identity: stable,
        device: s["device"]["name"].as_str().unwrap_or("").into(),
        device_type: s["device"]["type"].as_str().unwrap_or("").into(),
        model: model.unwrap_or_else(|| "Model okunamadı".into()),
        serial,
        firmware: text(&s["firmware_version"]),
        kind: kind.into(),
        protocol,
        metrics,
        health: h,
        smart,
        alerts,
        capabilities: json!({"self_test": s.pointer("/ata_smart_data/capabilities/self_tests_supported") == Some(&json!(true)) || s.pointer("/nvme_optional_admin_commands/self_test") == Some(&json!(true)), "trim": s["trim"]["supported"], "logs": s["smart_support"]["available"]}),
        interface: s
            .pointer("/interface_speed/current/string")
            .and_then(text)
            .or_else(|| {
                s.pointer("/nvme_version/string")
                    .and_then(text)
                    .map(format_nvme_interface)
            })
            .or_else(|| s.pointer("/sata_version/string").and_then(text)),
        logical_block_size: s["logical_block_size"].as_u64(),
        physical_block_size: s["physical_block_size"].as_u64(),
        rotation_rate: s["rotation_rate"].as_u64(),
        nvme_eui64,
        source_time: s["local_time"]["time_t"].as_i64(),
        os_disk_number: None,
        os_serial: None,
        os_model: None,
        volumes: Vec::new(),
        bus_link,
        optimization: Value::Null,
        read_incomplete,
        read_warning,
    }
}

fn parse_u128_value(value: &Value) -> Option<u128> {
    value
        .as_str()
        .and_then(|s| s.trim().parse::<u128>().ok())
        .or_else(|| unsigned(value).and_then(|s| s.parse::<u128>().ok()))
}

fn parse_counter(sample: &Value, key: &str, low16: bool) -> Option<u64> {
    let raw = sample.get("counters")?.get(key)?;
    let n = parse_u128_value(raw)?;
    if low16 {
        Some((n & 0xffff) as u64)
    } else {
        Some(n.min(u128::from(u64::MAX)) as u64)
    }
}

fn parse_counter_delta(first: &Value, last: &Value, key: &str, low16: bool) -> Option<i64> {
    let a = parse_counter(first, key, low16)?;
    let b = parse_counter(last, key, low16)?;
    let diff = i128::from(b) - i128::from(a);
    Some(diff.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64)
}

/// Pure, deterministic predictive health and SSD lifespan forecast derived solely from real
/// SQLite SMART history. Requires at least 2 distinct timestamps at least 1 hour apart; never
/// fabricates projections from insufficient samples.
pub fn compute_health_forecast(disk: &Disk, history: &[Value]) -> Value {
    let mut samples: Vec<(DateTime<Utc>, &Value)> = history
        .iter()
        .filter_map(|v| {
            let ts = v
                .get("time")
                .and_then(Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())?
                .with_timezone(&Utc);
            Some((ts, v))
        })
        .collect();
    samples.sort_by_key(|(ts, _)| *ts);

    let mut distinct_ts: Vec<i64> = samples.iter().map(|(ts, _)| ts.timestamp()).collect();
    distinct_ts.dedup();
    let sample_count = samples.len();
    let span_secs = match (samples.first(), samples.last()) {
        (Some((first, _)), Some((last, _))) => {
            last.signed_duration_since(*first).num_seconds().max(0)
        }
        _ => 0,
    };
    let span_hours = ((span_secs as f64 / 3600.0) * 10.0).round() / 10.0;
    let base_risk =
        if disk.health.status == "critical" || disk.health.percent.is_some_and(|p| p <= 20) {
            "critical"
        } else if disk.health.status == "warning" {
            "warning"
        } else {
            "ok"
        };

    if distinct_ts.len() < 2 || span_secs < 3600 {
        return json!({
            "ready": false,
            "sample_count": sample_count,
            "span_hours": span_hours,
            "daily_written_gb": Value::Null,
            "estimated_remaining_years": Value::Null,
            "pending_delta_7d": Value::Null,
            "reallocated_delta_7d": Value::Null,
            "uncorrectable_delta_7d": Value::Null,
            "temp_avg_7d": Value::Null,
            "risk_level": base_risk,
            "summary_tr": "Öngörü için en az 1 saat aralıklı 2 ölçüm geçmişi gerekiyor.",
            "advice_tr": "Gerçek SMART ölçümleri geçmişte biriktikçe günlük yazma hızı, tahmini kalan SSD ömrü ve 7 günlük hata eğilimi burada hesaplanır."
        });
    }

    let latest_ts = samples.last().unwrap().0;
    let cutoff_7d = latest_ts - Duration::days(7);
    let window_7d: Vec<&(DateTime<Utc>, &Value)> =
        samples.iter().filter(|(ts, _)| *ts >= cutoff_7d).collect();
    let first_7d = if window_7d.len() >= 2 {
        window_7d[0].1
    } else {
        samples
            .iter()
            .rev()
            .find(|(ts, _)| *ts < cutoff_7d)
            .map(|(_, v)| *v)
            .unwrap_or(samples[0].1)
    };
    let last_7d = samples.last().unwrap().1;

    let pending_delta_7d = parse_counter_delta(first_7d, last_7d, "197", true).unwrap_or(0);
    let reallocated_delta_7d = parse_counter_delta(first_7d, last_7d, "5", true).unwrap_or(0);
    let uncorrectable_delta_7d = parse_counter_delta(first_7d, last_7d, "198", true)
        .or_else(|| parse_counter_delta(first_7d, last_7d, "media_errors", false))
        .unwrap_or(0);

    let temps: Vec<f64> = window_7d
        .iter()
        .filter_map(|(_, v)| v["temperature_c"].as_f64())
        .filter(|t| t.is_finite() && *t > -50.0 && *t < 150.0)
        .collect();
    let temp_avg_7d = if temps.is_empty() {
        None
    } else {
        let avg = temps.iter().sum::<f64>() / temps.len() as f64;
        Some((avg * 10.0).round() / 10.0)
    };

    let written_points: Vec<(DateTime<Utc>, u128)> = samples
        .iter()
        .filter_map(|(ts, v)| parse_u128_value(&v["written_bytes"]).map(|w| (*ts, w)))
        .collect();
    let raw_daily_written_gb = match (written_points.first(), written_points.last()) {
        (Some((t0, w0)), Some((t1, w1))) => {
            let dt = t1.signed_duration_since(*t0).num_seconds();
            if dt >= 3600 && w1 >= w0 {
                let days = dt as f64 / 86_400.0;
                let gb = (w1 - w0) as f64 / 1_000_000_000.0;
                let rate = gb / days;
                rate.is_finite().then_some(rate)
            } else {
                None
            }
        }
        _ => None,
    };
    let daily_written_gb = raw_daily_written_gb.map(|g| (g * 100.0).round() / 100.0);

    let is_hdd = disk.kind == "HDD";
    let estimated_remaining_years = if !is_hdd {
        match (disk.health.percent, raw_daily_written_gb) {
            (Some(rem_pct), Some(daily_gb)) if rem_pct > 0 && daily_gb > 0.0 => {
                let used_pct = disk.metrics["percentage_used"]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .unwrap_or_else(|| f64::from(100u32.saturating_sub(rem_pct)));
                let current_written = parse_u128_value(&disk.metrics["written_bytes"])
                    .or_else(|| written_points.last().map(|(_, w)| *w));
                if used_pct >= 1.0 {
                    current_written.filter(|w| *w > 0).and_then(|total_w| {
                        let total_gb = total_w as f64 / 1_000_000_000.0;
                        let gb_per_pct = total_gb / used_pct;
                        let remaining_gb = gb_per_pct * f64::from(rem_pct);
                        let years = remaining_gb / daily_gb / 365.25;
                        years
                            .is_finite()
                            .then(|| (years.clamp(0.0, 99.9) * 10.0).round() / 10.0)
                    })
                } else {
                    let hp_points: Vec<(DateTime<Utc>, u64)> = samples
                        .iter()
                        .filter_map(|(ts, v)| v["health_percent"].as_u64().map(|p| (*ts, p)))
                        .collect();
                    match (hp_points.first(), hp_points.last()) {
                        (Some((t0, h0)), Some((t1, h1)))
                            if h0 > h1 && t1.signed_duration_since(*t0).num_seconds() >= 3600 =>
                        {
                            let days =
                                t1.signed_duration_since(*t0).num_seconds() as f64 / 86_400.0;
                            let pct_per_day = (h0 - h1) as f64 / days;
                            let years = f64::from(rem_pct) / pct_per_day / 365.25;
                            years
                                .is_finite()
                                .then(|| (years.clamp(0.0, 99.9) * 10.0).round() / 10.0)
                        }
                        _ => None,
                    }
                }
            }
            _ => None,
        }
    } else {
        None
    };

    let hot_threshold = if is_hdd { 50.0 } else { 55.0 };
    let risk_level = if base_risk == "critical"
        || pending_delta_7d > 0
        || uncorrectable_delta_7d > 0
        || reallocated_delta_7d >= 5
        || estimated_remaining_years.is_some_and(|y| y < 0.5)
    {
        "critical"
    } else if base_risk == "warning"
        || disk.health.percent.is_some_and(|p| p <= 40)
        || reallocated_delta_7d > 0
        || temp_avg_7d.is_some_and(|t| t >= hot_threshold)
        || estimated_remaining_years.is_some_and(|y| y < 2.0)
    {
        "warning"
    } else {
        "ok"
    };

    let (summary_tr, advice_tr): (String, String) = if risk_level == "critical" {
        let mut reasons = Vec::new();
        if pending_delta_7d > 0 {
            reasons.push(format!("son 7 günde +{pending_delta_7d} bekleyen sektör"));
        }
        if uncorrectable_delta_7d > 0 {
            reasons.push(format!(
                "son 7 günde +{uncorrectable_delta_7d} düzeltilemeyen / medya hatası"
            ));
        }
        if reallocated_delta_7d >= 5 {
            reasons.push(format!(
                "son 7 günde +{reallocated_delta_7d} yeniden eşlenen sektör"
            ));
        }
        if let Some(y) = estimated_remaining_years.filter(|y| *y < 0.5) {
            reasons.push(format!("kalan tahmini SSD dayanımı ~{y:.1} yıl"));
        }
        if reasons.is_empty() {
            reasons.push("kritik sağlık eşiği aşıldı".into());
        }
        (
            format!("Yüksek risk tespit edildi: {}.", reasons.join(", ")),
            if is_hdd {
                "Önemli verilerinizi hemen başka bir diske yedekleyin, Tam Yüzey Taraması yapın ve disk değişimini planlayın.".into()
            } else {
                "Önemli verilerinizi hemen başka bir diske yedekleyin ve SSD değişimini planlayın."
                    .into()
            },
        )
    } else if risk_level == "warning" {
        let mut notes = Vec::new();
        if reallocated_delta_7d > 0 {
            notes.push(format!(
                "son 7 günde +{reallocated_delta_7d} yeniden eşlenen sektör"
            ));
        }
        if let Some(y) = estimated_remaining_years.filter(|y| *y < 2.0) {
            notes.push(format!("tahmini kalan dayanım ~{y:.1} yıl"));
        }
        if let Some(t) = temp_avg_7d.filter(|t| *t >= hot_threshold) {
            notes.push(format!("7 günlük ortalama sıcaklık {t:.1} °C"));
        }
        if let Some(gb) = daily_written_gb {
            notes.push(format!("günlük ortalama {gb:.1} GB yazma"));
        }
        let summary = if notes.is_empty() {
            "Diskte yakından izlenmesi gereken uyarı sinyali var.".into()
        } else {
            format!("Dikkat gerektiren eğilim: {}.", notes.join(" · "))
        };
        let advice: String = if temp_avg_7d.is_some_and(|t| t >= hot_threshold) {
            "Kasa içi hava akışını veya M.2 soğutucuyu kontrol edin ve güncel yedeğinizi doğrulayın.".into()
        } else if is_hdd {
            "Güncel yedeğinizi doğrulayın; hata artışı sürerse Tam Yüzey Taraması ve Nokta Atışı Onarım uygulayın.".into()
        } else {
            "Güncel yedeğinizi doğrulayın; gereksiz ağır yazma yüklerini azaltmak SSD ömrünü uzatır.".into()
        };
        (summary, advice)
    } else {
        let summary = match (daily_written_gb, estimated_remaining_years) {
            (Some(gb), Some(years)) => format!(
                "Günlük ortalama {gb:.1} GB yazma hızıyla tahmini kalan dayanım ~{years:.1} yıl; son 7 günde yeni hata yok."
            ),
            (Some(gb), None) => format!(
                "Günlük ortalama {gb:.1} GB yazma ölçüldü; son 7 günde yeni sektör veya medya hatası yok."
            ),
            _ => match temp_avg_7d {
                Some(t) => format!(
                    "Son 7 günde yeni hata artışı gözlenmedi; ortalama çalışma sıcaklığı {t:.1} °C."
                ),
                None => "Son 7 günde yeni hata artışı gözlenmedi; disk eğilimi kararlı.".into(),
            },
        };
        (
            summary,
            "Belirgin bir aşınma veya yüzey riski görünmüyor; düzenli yedekleme alışkanlığını sürdürün.".into(),
        )
    };

    json!({
        "ready": true,
        "sample_count": sample_count,
        "span_hours": span_hours,
        "daily_written_gb": daily_written_gb,
        "estimated_remaining_years": estimated_remaining_years,
        "pending_delta_7d": pending_delta_7d,
        "reallocated_delta_7d": reallocated_delta_7d,
        "uncorrectable_delta_7d": uncorrectable_delta_7d,
        "temp_avg_7d": temp_avg_7d,
        "risk_level": risk_level,
        "summary_tr": summary_tr,
        "advice_tr": advice_tr
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_nvme_json(ver: &str) -> Value {
        json!({
            "model_name": "Test NVMe 512G",
            "serial_number": "SN123456",
            "device": { "name": "/dev/sdb", "type": "nvme", "protocol": "NVMe" },
            "smartctl": { "exit_status": 0 },
            "smart_status": { "passed": true },
            "nvme_version": { "string": ver },
            "user_capacity": { "bytes": 512_000_000_000u64 },
            "nvme_smart_health_information_log": {
                "critical_warning": 0,
                "temperature": 42,
                "available_spare": 100,
                "available_spare_threshold": 10,
                "percentage_used": 27,
                "data_units_written": 105_468_750u64,
                "data_units_read": 50_000_000u64,
                "media_errors": 0,
                "num_err_log_entries": 0
            }
        })
    }

    fn sample_hdd_json() -> Value {
        json!({
            "model_name": "Test HDD 1TB",
            "serial_number": "WD123456",
            "rotation_rate": 7200,
            "device": { "name": "/dev/sda", "type": "ata", "protocol": "ATA" },
            "smartctl": { "exit_status": 0 },
            "smart_status": { "passed": true },
            "interface_speed": { "current": { "string": "6.0 Gb/s", "units_per_second": 60 }, "max": { "string": "6.0 Gb/s", "units_per_second": 60 } },
            "temperature": { "current": 36 },
            "ata_smart_attributes": {
                "table": [
                    { "id": 5, "name": "Reallocated_Sector_Ct", "value": 200, "worst": 200, "thresh": 140, "when_failed": "", "raw": { "value": 0 } },
                    { "id": 7, "name": "Seek_Error_Rate", "value": 200, "worst": 200, "thresh": 0, "when_failed": "", "raw": { "value": 0 } },
                    { "id": 10, "name": "Spin_Retry_Count", "value": 100, "worst": 100, "thresh": 0, "when_failed": "", "raw": { "value": 0 } },
                    { "id": 196, "name": "Reallocated_Event_Count", "value": 200, "worst": 200, "thresh": 0, "when_failed": "", "raw": { "value": 0 } },
                    { "id": 197, "name": "Current_Pending_Sector", "value": 200, "worst": 200, "thresh": 0, "when_failed": "", "raw": { "value": 0 } },
                    { "id": 198, "name": "Offline_Uncorrectable", "value": 200, "worst": 200, "thresh": 0, "when_failed": "", "raw": { "value": 0 } }
                ]
            }
        })
    }

    #[test]
    fn bare_nvme_interface_versions_are_prefixed_without_affecting_sata() {
        assert_eq!(
            normalise(&sample_nvme_json("1.3")).interface.as_deref(),
            Some("NVMe 1.3")
        );
        assert_eq!(
            normalise(&sample_nvme_json("1.4")).interface.as_deref(),
            Some("NVMe 1.4")
        );
        assert_eq!(
            normalise(&sample_nvme_json("2.0")).interface.as_deref(),
            Some("NVMe 2.0")
        );
        assert_eq!(
            normalise(&sample_nvme_json("NVMe 1.4"))
                .interface
                .as_deref(),
            Some("NVMe 1.4")
        );
        assert_eq!(
            normalise(&sample_hdd_json()).interface.as_deref(),
            Some("6.0 Gb/s")
        );
    }

    #[test]
    fn health_forecast_requires_at_least_two_samples_one_hour_apart() {
        let ssd = normalise(&sample_nvme_json("1.3"));
        assert_eq!(compute_health_forecast(&ssd, &[])["ready"], false);
        assert_eq!(
            compute_health_forecast(
                &ssd,
                &[json!({"time": "2026-10-21T06:00:00Z", "temperature_c": 42})]
            )["ready"],
            false
        );
        let too_close = vec![
            json!({"time": "2026-10-21T06:00:00Z", "written_bytes": "54000000000000"}),
            json!({"time": "2026-10-21T06:45:00Z", "written_bytes": "54010000000000"}),
        ];
        let fc = compute_health_forecast(&ssd, &too_close);
        assert_eq!(fc["ready"], false);
        assert!(fc["daily_written_gb"].is_null());
        assert!(fc["estimated_remaining_years"].is_null());
        assert!(fc["summary_tr"].as_str().unwrap().contains("1 saat"));
    }

    #[test]
    fn health_forecast_computes_ssd_daily_writes_and_remaining_years() {
        // percentage_used = 27 -> health.percent = 73; data_units_written = 105_468_750 -> 54_000 GB
        // gb_per_pct = 54_000 / 27 = 2_000 GB/%; remaining = 73 * 2_000 = 146_000 GB
        // 40 GB/day -> 146_000 / 40 / 365.25 ~= 10.0 years
        let ssd = normalise(&sample_nvme_json("1.3"));
        let history = vec![
            json!({
                "time": "2026-10-15T00:00:00Z",
                "temperature_c": 40,
                "written_bytes": "53800000000000",
                "health_percent": 73,
                "counters": { "media_errors": "0" }
            }),
            json!({
                "time": "2026-10-20T00:00:00Z",
                "temperature_c": 44,
                "written_bytes": "54000000000000",
                "health_percent": 73,
                "counters": { "media_errors": "0" }
            }),
        ];
        let fc = compute_health_forecast(&ssd, &history);
        assert_eq!(fc["ready"], true);
        assert_eq!(fc["daily_written_gb"], 40.0);
        assert_eq!(fc["estimated_remaining_years"], 10.0);
        assert_eq!(fc["temp_avg_7d"], 42.0);
        assert_eq!(fc["pending_delta_7d"], 0);
        assert_eq!(fc["reallocated_delta_7d"], 0);
        assert_eq!(fc["uncorrectable_delta_7d"], 0);
        assert_eq!(fc["risk_level"], "ok");
    }

    #[test]
    fn health_forecast_detects_hdd_7d_sector_growth_and_never_fabricates_ssd_years() {
        let hdd = normalise(&sample_hdd_json());
        let history = vec![
            json!({
                "time": "2026-10-18T10:00:00Z",
                "temperature_c": 36,
                "counters": { "5": "2", "197": "0", "198": "0" }
            }),
            json!({
                "time": "2026-10-21T10:00:00Z",
                "temperature_c": 38,
                "counters": { "5": "4", "197": "3", "198": "1" }
            }),
        ];
        let fc = compute_health_forecast(&hdd, &history);
        assert_eq!(fc["ready"], true);
        assert!(fc["estimated_remaining_years"].is_null());
        assert_eq!(fc["reallocated_delta_7d"], 2);
        assert_eq!(fc["pending_delta_7d"], 3);
        assert_eq!(fc["uncorrectable_delta_7d"], 1);
        assert_eq!(fc["temp_avg_7d"], 37.0);
        assert_eq!(fc["risk_level"], "critical");
        assert!(fc["advice_tr"].as_str().unwrap().contains("yedekleyin"));
    }

    #[test]
    fn health_forecast_flags_nvme_media_error_growth_and_handles_counter_reset() {
        let ssd = normalise(&sample_nvme_json("1.4"));
        let history = vec![
            json!({
                "time": "2026-10-20T00:00:00Z",
                "temperature_c": 45,
                "written_bytes": "60000000000000",
                "counters": { "media_errors": "1" }
            }),
            json!({
                "time": "2026-10-21T00:00:00Z",
                "temperature_c": 47,
                "written_bytes": "54000000000000",
                "counters": { "media_errors": "3" }
            }),
        ];
        let fc = compute_health_forecast(&ssd, &history);
        assert_eq!(fc["ready"], true);
        assert!(fc["daily_written_gb"].is_null());
        assert!(fc["estimated_remaining_years"].is_null());
        assert_eq!(fc["uncorrectable_delta_7d"], 2);
        assert_eq!(fc["risk_level"], "critical");
    }
}
