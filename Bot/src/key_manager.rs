// ============================================================================
//  KEY MANAGER v1.0 — Şifreli Private Key Yönetimi
//
//  Güvenlik Katmanları:
//  ✓ AES-256-GCM ile şifreleme (AEAD — kimlik doğrulamalı şifreleme)
//  ✓ PBKDF2-HMAC-SHA256 ile parola tabanlı anahtar türetimi (600K iterasyon)
//  ✓ Zeroize ile güvenli hafıza temizliği (drop sonrası RAM'de iz kalmaz)
//  ✓ Disk üzerinde düz metin private key ASLA bulunmaz
//
//  Kullanım Modları:
//  1. Şifreli Keystore Dosyası (keystore.enc) — En güvenli
//     - İlk kurulumda: encrypt_and_save() ile oluştur
//     - Runtime: load_and_decrypt() ile bellek içi çöz
//     - Parola: KEY_PASSWORD env var veya terminal prompt
//
//  2. Ortam Değişkeni (PRIVATE_KEY) — Geriye uyumluluk
//     - Güvenlik UYARISI loglanır
//     - Yalnızca geçiş dönemi için
//
//  Keystore Dosya Formatı (JSON):
//  {
//    "version": 1,
//    "kdf": "pbkdf2-hmac-sha256",
//    "kdf_iterations": 600000,
//    "salt": "<hex 32 byte>",
//    "nonce": "<hex 12 byte>",
//    "ciphertext": "<hex>",
//    "tag": "<hex 16 byte>"
//  }
// ============================================================================

// NOT: generic-array 0.14 → 1.x geçiş sürecinde aes-gcm 0.10 upstream
// deprecated uyarıları üretir. Bug değil, ekosistem geçiş uyarısıdır.
// aes-gcm generic-array 1.x desteği eklediğinde bu kaldırılacak.
#![allow(deprecated)]

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use hmac::Hmac;
use pbkdf2::pbkdf2;
use sha2::Sha256;
use rand::RngCore;
use zeroize::Zeroizing;
use eyre::Result;
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Sabitler
// ─────────────────────────────────────────────────────────────────────────────

/// PBKDF2 iterasyon sayısı — OWASP 2024 önerisi (SHA-256 için ≥600K)
const PBKDF2_ITERATIONS: u32 = 600_000;

/// Salt boyutu (byte)
const SALT_SIZE: usize = 32;

/// AES-GCM nonce boyutu (byte)
const NONCE_SIZE: usize = 12;

/// Keystore dosya versiyonu
const KEYSTORE_VERSION: u32 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Keystore Yapısı (JSON serileştirme için)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct KeystoreFile {
    version: u32,
    kdf: String,
    kdf_iterations: u32,
    salt: String,     // hex-encoded
    nonce: String,    // hex-encoded
    ciphertext: String, // hex-encoded (ciphertext + auth tag)
}

// ─────────────────────────────────────────────────────────────────────────────
// Key Manager
// ─────────────────────────────────────────────────────────────────────────────

/// Şifreli private key yöneticisi.
///
/// Private key hiçbir zaman diske düz metin olarak yazılmaz.
/// Bellekte tutulur ve drop edildiğinde zeroize ile güvenli şekilde silinir.
pub struct KeyManager {
    /// Çözülmüş private key (bellekte, zeroize destekli)
    decrypted_key: Option<Zeroizing<String>>,
    /// Anahtar kaynağı (log/debug için)
    source: KeySource,
}

/// Private key'in nereden yüklendiği
#[derive(Debug, Clone)]
pub enum KeySource {
    /// Şifreli keystore dosyasından
    EncryptedKeystore(String),
    /// Ortam değişkeninden (güvenli DEĞİL — uyarı verilir)
    EnvironmentVariable,
    /// Henüz yüklenmedi
    None,
}

impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySource::EncryptedKeystore(path) => write!(f, "Şifreli Keystore ({})", path),
            KeySource::EnvironmentVariable => write!(f, "Ortam Değişkeni (GÜVENSİZ)"),
            KeySource::None => write!(f, "Yüklenmedi"),
        }
    }
}

impl KeyManager {
    // ─────────────────────────────────────────────────────────────────────
    // Oluşturma ve Yükleme
    // ─────────────────────────────────────────────────────────────────────

    /// Boş KeyManager oluştur
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            decrypted_key: None,
            source: KeySource::None,
        }
    }

    /// Private key'i şifrele ve dosyaya kaydet.
    ///
    /// # Güvenlik
    /// - Parola → PBKDF2-HMAC-SHA256 (600K iter) → 32-byte AES key
    /// - AES-256-GCM ile şifreleme (AEAD — bütünlük + gizlilik)
    /// - Rastgele 32-byte salt + 12-byte nonce
    ///
    /// # Kullanım
    /// ```ignore
    /// KeyManager::encrypt_and_save("0xabc...private_key", "güçlü_parola", "keystore.enc")?;
    /// ```
    pub fn encrypt_and_save(private_key: &str, password: &str, path: &str) -> Result<()> {
        // 1. Rastgele salt ve nonce üret
        let mut salt = [0u8; SALT_SIZE];
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        rand::thread_rng().fill_bytes(&mut salt);
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        // 2. Paroladan AES-256 anahtarı türet (PBKDF2)
        let mut derived_key = Zeroizing::new([0u8; 32]);
        pbkdf2::<Hmac<Sha256>>(
            password.as_bytes(),
            &salt,
            PBKDF2_ITERATIONS,
            derived_key.as_mut(),
        ).map_err(|e| eyre::eyre!("PBKDF2 anahtar türetme hatası: {:?}", e))?;

        // 3. AES-256-GCM ile şifrele
        let cipher = Aes256Gcm::new_from_slice(derived_key.as_ref())
            .map_err(|e| eyre::eyre!("AES-256-GCM oluşturma hatası: {}", e))?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, private_key.as_bytes())
            .map_err(|e| eyre::eyre!("Şifreleme hatası: {}", e))?;

        // 4. Keystore dosyasını oluştur
        let keystore = KeystoreFile {
            version: KEYSTORE_VERSION,
            kdf: "pbkdf2-hmac-sha256".into(),
            kdf_iterations: PBKDF2_ITERATIONS,
            salt: hex::encode(&salt),
            nonce: hex::encode(&nonce_bytes),
            ciphertext: hex::encode(&ciphertext),
        };

        let json = serde_json::to_string_pretty(&keystore)
            .map_err(|e| eyre::eyre!("JSON serileştirme hatası: {}", e))?;

        std::fs::write(path, json)
            .map_err(|e| eyre::eyre!("Keystore dosyası yazma hatası: {}", e))?;

        Ok(())
    }

    /// Şifreli keystore dosyasından private key'i yükle ve çöz.
    ///
    /// # Güvenlik
    /// - Çözülen anahtar sadece bellekte tutulur (Zeroizing<String>)
    /// - Drop edildiğinde otomatik zeroize
    /// - Disk erişimi olan saldırgan anahtarı okuyamaz
    pub fn load_from_keystore(path: &str, password: &str) -> Result<Self> {
        // 1. Dosyayı oku ve JSON çözümle
        let json = std::fs::read_to_string(path)
            .map_err(|e| eyre::eyre!("Keystore dosyası okunamadı ({}): {}", path, e))?;

        let keystore: KeystoreFile = serde_json::from_str(&json)
            .map_err(|e| eyre::eyre!("Keystore JSON çözümleme hatası: {}", e))?;

        // Versiyon kontrolü
        if keystore.version != KEYSTORE_VERSION {
            return Err(eyre::eyre!(
                "Desteklenmeyen keystore versiyonu: {} (beklenen: {})",
                keystore.version,
                KEYSTORE_VERSION
            ));
        }

        // 2. Hex decode
        let salt = hex::decode(&keystore.salt)
            .map_err(|e| eyre::eyre!("Salt hex çözümleme hatası: {}", e))?;
        let nonce_bytes = hex::decode(&keystore.nonce)
            .map_err(|e| eyre::eyre!("Nonce hex çözümleme hatası: {}", e))?;
        let ciphertext = hex::decode(&keystore.ciphertext)
            .map_err(|e| eyre::eyre!("Ciphertext hex çözümleme hatası: {}", e))?;

        // 3. PBKDF2 ile anahtar türet
        let mut derived_key = Zeroizing::new([0u8; 32]);
        pbkdf2::<Hmac<Sha256>>(
            password.as_bytes(),
            &salt,
            keystore.kdf_iterations,
            derived_key.as_mut(),
        ).map_err(|e| eyre::eyre!("PBKDF2 anahtar türetme hatası: {:?}", e))?;

        // 4. AES-256-GCM ile çöz
        let cipher = Aes256Gcm::new_from_slice(derived_key.as_ref())
            .map_err(|e| eyre::eyre!("AES-256-GCM oluşturma hatası: {}", e))?;

        if nonce_bytes.len() != NONCE_SIZE {
            return Err(eyre::eyre!("Geçersiz nonce boyutu: {}", nonce_bytes.len()));
        }
        let nonce = Nonce::from_slice(&nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|_| eyre::eyre!(
                "Şifre çözme başarısız! Yanlış parola veya bozuk keystore dosyası."
            ))?;

        let key_string = String::from_utf8(plaintext)
            .map_err(|_| eyre::eyre!("Çözülen anahtar geçerli UTF-8 değil"))?;

        Ok(Self {
            decrypted_key: Some(Zeroizing::new(key_string)),
            source: KeySource::EncryptedKeystore(path.to_string()),
        })
    }

    /// Ortam değişkeninden private key oku (geriye uyumluluk).
    ///
    /// # Güvenlik Uyarısı
    /// Bu mod GÜVENLİ DEĞİLDİR. Disk üzerinde .env dosyasında düz metin
    /// private key bulunur. Mümkün olan en kısa sürede şifreli keystore'a
    /// geçiş yapılmalıdır.
    pub fn load_from_env(env_key: &str) -> Result<Self> {
        let key = std::env::var(env_key)
            .ok()
            .filter(|k| !k.is_empty() && k != "your-private-key-here");

        match key {
            Some(k) => Ok(Self {
                decrypted_key: Some(Zeroizing::new(k)),
                source: KeySource::EnvironmentVariable,
            }),
            None => Ok(Self {
                decrypted_key: None,
                source: KeySource::None,
            }),
        }
    }

    /// Otomatik yükleme: Önce keystore dene, yoksa env var'a düş.
    ///
    /// Öncelik sırası:
    /// 1. KEYSTORE_PATH env var → şifreli dosyadan yükle
    /// 2. PRIVATE_KEY env var → düz metin (UYARI ile)
    /// 3. Hiçbiri → key yok
    pub fn auto_load() -> Result<Self> {
        // 1. Keystore dosyası var mı?
        let keystore_path = std::env::var("KEYSTORE_PATH")
            .ok()
            .filter(|p| !p.is_empty());

        if let Some(ref path) = keystore_path {
            if Path::new(path).exists() {
                // Parolayı al: env var veya terminal prompt
                let password = Self::get_password()?;
                return Self::load_from_keystore(path, &password);
            }
        }

        // 2. Ortam değişkeninden oku (geriye uyumluluk)
        let manager = Self::load_from_env("PRIVATE_KEY")?;
        if manager.has_key() {
            eprintln!(
                "  ⚠️  GÜVENLİK UYARISI: Private key düz metin ortam değişkeninden okundu!"
            );
            eprintln!(
                "  ⚠️  Şifreli keystore'a geçmek için: cargo run -- --encrypt-key"
            );
        }

        Ok(manager)
    }

    // ─────────────────────────────────────────────────────────────────────
    // Erişim
    // ─────────────────────────────────────────────────────────────────────

    /// Private key'e erişim (bellekteki çözülmüş değer)
    pub fn private_key(&self) -> Option<&str> {
        self.decrypted_key.as_ref().map(|k| k.as_str())
    }

    /// Key yüklü mü?
    pub fn has_key(&self) -> bool {
        self.decrypted_key.is_some()
    }

    /// Anahtar kaynağı
    pub fn source(&self) -> &KeySource {
        &self.source
    }

    // ─────────────────────────────────────────────────────────────────────
    // Yardımcılar
    // ─────────────────────────────────────────────────────────────────────

    /// Parolayı ortam değişkeninden veya terminal'den al
    fn get_password() -> Result<String> {
        // Önce env var dene (CI/CD ve otomatik deploy için)
        if let Ok(pwd) = std::env::var("KEY_PASSWORD") {
            if !pwd.is_empty() {
                return Ok(pwd);
            }
        }

        // Terminal'den güvenli parola girişi
        eprint!("🔐 Keystore parolassı: ");
        rpassword::read_password()
            .map_err(|e| eyre::eyre!("Parola okuma hatası: {}", e))
    }

    /// CLI: Private key'i şifreleyip keystore dosyasına kaydet
    ///
    /// # Kullanım
    /// ```ignore
    /// KeyManager::cli_encrypt_key()?;
    /// ```
    pub fn cli_encrypt_key() -> Result<()> {
        println!("\n🔐 Private Key Şifreleme Aracı");
        println!("─────────────────────────────────────────");
        println!("Bu araç private key'inizi AES-256-GCM ile şifreler.");
        println!("Şifreli dosya güvenle disk üzerinde saklanabilir.\n");

        // Private key al
        eprint!("Private key (0x...): ");
        let private_key = rpassword::read_password()
            .map_err(|e| eyre::eyre!("Private key okuma hatası: {}", e))?;

        if private_key.is_empty() {
            return Err(eyre::eyre!("Private key boş olamaz!"));
        }

        // Parola al (iki kez, doğrulama)
        eprint!("Şifreleme parolası: ");
        let password = rpassword::read_password()
            .map_err(|e| eyre::eyre!("Parola okuma hatası: {}", e))?;
        eprint!("Parolayı tekrar girin: ");
        let password_confirm = rpassword::read_password()
            .map_err(|e| eyre::eyre!("Parola okuma hatası: {}", e))?;

        if password != password_confirm {
            return Err(eyre::eyre!("Parolalar eşleşmiyor!"));
        }

        if password.len() < 8 {
            return Err(eyre::eyre!("Parola en az 8 karakter olmalıdır!"));
        }

        // Dosya yolu
        let path = std::env::var("KEYSTORE_PATH")
            .unwrap_or_else(|_| "keystore.enc".into());

        // Şifrele ve kaydet
        println!("\n⏳ Anahtar türetiliyor (PBKDF2, {} iterasyon)...", PBKDF2_ITERATIONS);
        Self::encrypt_and_save(&private_key, &password, &path)?;

        println!("✅ Keystore başarıyla oluşturuldu: {}", path);
        println!("\n📋 .env dosyanıza ekleyin:");
        println!("   KEYSTORE_PATH={}", path);
        println!("   KEY_PASSWORD=<parolanız>  (veya runtime'da prompt)");
        println!("\n⚠️  PRIVATE_KEY satırını .env'den SİLMEYİ unutmayın!");

        Ok(())
    }
}

impl Drop for KeyManager {
    fn drop(&mut self) {
        // Zeroizing<String> otomatik olarak belleği temizler.
        // Ek güvenlik: source bilgisini de temizle
        self.source = KeySource::None;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Testler
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let password = "test_password_123";
        let path = "test_keystore_roundtrip.enc";

        // Şifrele
        KeyManager::encrypt_and_save(private_key, password, path).unwrap();
        assert!(Path::new(path).exists(), "Keystore dosyası oluşmalı");

        // Çöz
        let manager = KeyManager::load_from_keystore(path, password).unwrap();
        assert_eq!(
            manager.private_key().unwrap(),
            private_key,
            "Çözülen key orijinalle eşleşmeli"
        );

        // Temizle
        fs::remove_file(path).ok();
    }

    #[test]
    fn test_wrong_password_fails() {
        let private_key = "0xdeadbeef";
        let password = "correct_password";
        let wrong_password = "wrong_password";
        let path = "test_keystore_wrong_pwd.enc";

        KeyManager::encrypt_and_save(private_key, password, path).unwrap();

        let result = KeyManager::load_from_keystore(path, wrong_password);
        assert!(result.is_err(), "Yanlış parola ile çözme başarısız olmalı");

        fs::remove_file(path).ok();
    }

    #[test]
    fn test_corrupted_file_fails() {
        let path = "test_keystore_corrupt.enc";
        fs::write(path, "this is not valid json").unwrap();

        let result = KeyManager::load_from_keystore(path, "any_password");
        assert!(result.is_err(), "Bozuk dosya ile yükleme başarısız olmalı");

        fs::remove_file(path).ok();
    }

    #[test]
    fn test_empty_key_manager() {
        let manager = KeyManager::new();
        assert!(!manager.has_key());
        assert!(manager.private_key().is_none());
    }

    #[test]
    fn test_env_var_fallback() {
        // NONEXISTENT_KEY env var yok → key yüklenmemeli
        let manager = KeyManager::load_from_env("NONEXISTENT_TEST_KEY_12345").unwrap();
        assert!(!manager.has_key());
    }

    #[test]
    fn test_different_keys_produce_different_ciphertexts() {
        let password = "same_password";
        let path1 = "test_keystore_diff1.enc";
        let path2 = "test_keystore_diff2.enc";

        KeyManager::encrypt_and_save("key_one", password, path1).unwrap();
        KeyManager::encrypt_and_save("key_two", password, path2).unwrap();

        let json1 = fs::read_to_string(path1).unwrap();
        let json2 = fs::read_to_string(path2).unwrap();

        // Salt ve nonce rastgele → ciphertext farklı olmalı
        assert_ne!(json1, json2, "Farklı key'ler farklı ciphertext üretmeli");

        fs::remove_file(path1).ok();
        fs::remove_file(path2).ok();
    }
}
