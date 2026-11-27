import { test } from "node:test";
import assert from "node:assert/strict";
import {
  translateAlert,
  translateAppCategory,
  translateAppSource,
  translateAutopilotStep,
  translateBackend,
  translateBackendError,
  translateBlockText,
  translateEventMessage,
  translateJobPhase,
  translateOptimizationTitle,
  translatePinpointLabel,
  translateRecoveryText,
  translateScopeOrAdvice,
} from "../src/i18n.ts";
import { BACKEND_PAIRS } from "../src/i18n_backend_data.ts";
import { computePollSignature } from "../src/ui.ts";

const TURKISH = /[çğıöşüÇĞİÖŞÜ]/;
const noTurkish = (text: string) =>
  assert.ok(!TURKISH.test(text), `Turkish leaked into English text: ${text}`);

test("free-space alerts are translated for the English UI", () => {
  const alert = {
    code: "space-C",
    title: "C: boş alanı %10'un altında",
    detail: "Büyük dosyaları taşıyın veya Windows depolama ayarlarını inceleyin.",
  };
  const en = translateAlert(alert, "en");
  assert.equal(en.title, "C: free space below 10%");
  noTurkish(en.detail);
  assert.deepEqual(translateAlert(alert, "tr"), {
    title: alert.title,
    detail: alert.detail,
  });
});

test("pinpoint repair labels and results are fully translated", () => {
  for (const tr of [
    "Veri kaybı · dosya kısmen okunamıyor",
    "Kalıcı ölü sektör · chkdsk /r gerekli",
    "Sistem alanı · chkdsk /r önerilir",
    "Dosya kilitli · yeniden deneyin",
    "Tazelendi · veri yeni hücrelere yazıldı",
    "Boş alan · işlem gerekmez",
    "Zayıf sektör",
    "Okunamayan sektör",
    "Yavaş bölge",
    "Onarıldı",
    "Kalıcı olarak kapatıldı",
    "Onarılamadı · kapatıldı",
  ]) {
    const en = translatePinpointLabel(tr, "en");
    noTurkish(en);
    assert.notEqual(en, tr);
    assert.equal(translatePinpointLabel(tr, "tr"), tr);
  }
});

test("job progress text is translated (the 'applying safe profile' bug)", () => {
  assert.equal(translateJobPhase("Disk kimliği doğrulanıyor", "en"), "Verifying drive identity");
  assert.equal(translateJobPhase("Kopyalanıyor · film.mkv", "en"), "Copying · film.mkv");
  assert.equal(translateJobPhase("SHA-256 doğrulanıyor · film.mkv", "en"), "Verifying SHA-256 · film.mkv");
  assert.equal(translateJobPhase("12 dosya temizlendi", "en"), "12 files cleaned");
  assert.equal(
    translateJobPhase("1/4 · Windows TRIM (DisableDeleteNotify) doğrulanıyor", "en"),
    "1/4 · Verifying Windows TRIM (DisableDeleteNotify)",
  );
  assert.equal(translateJobPhase("Tamamlandı", "en"), "Completed");
  // Turkish mode and unknown text pass through untouched.
  assert.equal(translateJobPhase("Tamamlandı", "tr"), "Tamamlandı");
  assert.equal(translateJobPhase("Something custom", "en"), "Something custom");
  assert.equal(translateJobPhase(null, "en"), "");
});

test("autopilot steps, scopes and advice from saved Turkish records translate cleanly", () => {
  const guard =
    "Güvenlik uyarısı: HDD birleştirme (defrag /O) atlandı (SMART #197 bekleyen sektör). Arızalı diski zorlamamak için önce Tam Yüzey Taraması ve Nokta Atışı Onarım çalıştırın, ardından önemli verilerinizi yedekleyin.";
  const en = translateAutopilotStep(guard, "en");
  assert.match(en, /^Safety warning: HDD consolidation \(defrag \/O\) was skipped/);
  const scope = translateScopeOrAdvice(
    "Tam yüzey: 12.3 GB baştan sona okundu · 5 dk · 2 yavaş, 0 okunamayan parça.",
    "en",
  );
  assert.match(scope, /^Full surface: 12\.3 GB read end-to-end/);
  assert.equal(
    translateScopeOrAdvice("Son tam tarama sorun bulmadı; onarılacak bir şey yok.", "en"),
    "The last full scan found no problems; nothing to repair.",
  );
  assert.equal(translateBlockText("Dosya kimliği değişti.", "en"), "File identity changed.");
});

test("saved optimization titles and history events translate", () => {
  const t = (key: string, title: string) => translateOptimizationTitle({ key, title }, "en");
  assert.equal(t("write_cache:d1", "Yazma önbelleği · Samsung 980"), "Write cache · Samsung 980");
  assert.equal(t("op:d1:C", "SSD Over-Provisioning (%10) · C:"), "SSD Over-Provisioning (10%) · C:");
  assert.equal(t("tail:d1:D", "Ölü Kuyruk İzolasyonu (%5) · D:"), "Dead-Tail Isolation (5%) · D:");
  assert.equal(t("hibernation:system", "Hazırda Bekletme (hiberfil.sys)"), "Hibernation (hiberfil.sys)");
  assert.equal(
    t("pcie_aspm:system", "PCIe ASPM Güç Yönetimi (NVMe gecikme profili)"),
    "PCIe ASPM Power Management (NVMe latency profile)",
  );
  assert.equal(
    translateEventMessage({ code: "health-drop", message: "Sağlık / dayanım %98 → %91 düştü." }, "en"),
    "Health / endurance dropped from 98% to 91%.",
  );
  assert.equal(
    translateEventMessage({ code: "counter:197", message: "197 sayacı 0 → 8 arttı." }, "en"),
    "Counter 197 increased from 0 to 8.",
  );
});

test("deleted-app and recovery metadata is translated", () => {
  assert.equal(translateAppCategory("Araç", "en"), "Tool");
  assert.equal(translateAppCategory("Oyun", "en"), "Game");
  assert.equal(translateAppCategory("Uygulama", "en"), "App");
  assert.equal(translateAppCategory("", "en"), "App");
  assert.equal(translateAppCategory("Araç", "tr"), "Araç");
  for (const source of [
    "Güvenlik Duvarı",
    "VSS Gölge Kopya",
    "AppData Kalıntısı",
    "Kayıt Defteri",
    "Yetim Kayıt",
    "Kırık Kısayol (.lnk)",
    "Geri Dönüşüm Kutusu",
  ]) {
    noTurkish(translateAppSource(source, "en"));
  }
  assert.equal(translateRecoveryText("PNG Görsel (800×600)", "en"), "PNG Image (800×600)");
  assert.equal(translateRecoveryText("ZIP Arşivi", "en"), "ZIP Archive");
  assert.equal(
    translateRecoveryText(
      "Sektör yapısı ve imzası doğrulandı (Entropi: 7.91 bit/B, Bütünlük: %100).",
      "en",
    ),
    "Sector structure and signature verified (Entropy: 7.91 bit/B, Integrity: 100%).",
  );
  // A repair note prefixed to an AI summary is translated sentence by sentence.
  const combined = translateRecoveryText(
    "Eksik JPEG sonu (EOI) onarıldı; kurtarılan bölüm açılabilir hale getirildi. Sektör içeriği tamamen sıfır (üzerine yazılmış veya boş alan).",
    "en",
  );
  noTurkish(combined);
  assert.match(combined, /Missing JPEG end marker/);
  assert.equal(
    translateRecoveryText("Spotify · Ürün: Spotify. Dosya verisi boş veya TRIM ile sıfırlanmış; açılamaz.", "en"),
    "Spotify · Product: Spotify. File data is empty or zeroed by TRIM; cannot be opened.",
  );
});

test("backend errors and warnings translate, including the cancellation sentinel", () => {
  assert.equal(translateBackendError("İşlem iptal edildi.", "en"), "Operation cancelled.");
  assert.equal(translateBackendError("İşlem iptal edildi.", "tr"), "İşlem iptal edildi.");
  assert.equal(
    translateBackendError("/dev/sda: SMART okuması eksik (kod 4).", "en"),
    "/dev/sda: SMART read incomplete (code 4).",
  );
  assert.equal(
    translateBackendError(
      "Salt-okunur koruma etkin (benchmark engellendi). Yazma kilidini üst çubuktan veya Ayarlar'dan açın.",
      "en",
    ),
    "Read-Only Guard is active (benchmark blocked). Unlock write protection from the top bar or Settings.",
  );
  assert.equal(
    translateBackend("Kaydedilmiş ölçüm. Canlı Windows ve disk işlemleri kapalı.", "en"),
    "Recorded snapshot. Live Windows and disk operations are disabled.",
  );
  assert.equal(translateBackendError(null, "en"), "");
});

test("every backend translation pair is clean English and round-trips", () => {
  assert.ok(BACKEND_PAIRS.length > 300, "translation table looks empty");
  for (const [tr, en] of BACKEND_PAIRS) {
    noTurkish(en);
    assert.equal(translateBackend(tr, "tr"), tr, "Turkish mode must never rewrite text");
    if (!tr.includes("{")) {
      assert.equal(translateBackend(tr, "en"), en, `no exact translation for: ${tr}`);
    } else {
      const filled = tr.replace(/\{[^{}]*\}/g, "42");
      const translated = translateBackend(filled, "en");
      noTurkish(translated);
      assert.notEqual(translated, filled, `template was not translated: ${tr}`);
    }
  }
});

test("poll signature changes when the storage mode changes", () => {
  const opts = { page: "tools", selected: "d1", letter: "C", connectionError: "" };
  const installed = computePollSignature({ source: "live", portable: false, data_dir: "C:\\data" }, opts);
  const portable = computePollSignature({ source: "live", portable: true, data_dir: "D:\\app\\data" }, opts);
  assert.notEqual(installed, portable);
});
