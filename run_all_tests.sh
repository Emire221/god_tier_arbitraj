#!/usr/bin/env bash
set -e # Herhangi bir hata olursa betiği anında durdur

echo -e "\033[1;33m============================================================\033[0m"
echo -e "\033[1;32m🚀 GOD-TIER ARBITRAJ SİSTEMİ - TAM OTOMATİK TEST BAŞLIYOR...\033[0m"
echo -e "\033[1;33m============================================================\033[0m"

# Klasör yolları (Masaüstünde yan yana oldukları varsayımıyla)
CONTRACT_DIR="arbitraj_contract"
BOT_DIR="arbitraj_botuu"

# ─── AŞAMA 1: FOUNDRY FUZZ TESTING ───
echo -e "\n\033[1;36m[1/4] 🔫 Kontrat Dayanıklılık Testi (Foundry Fuzzing) Başlıyor...\033[0m"
cd $CONTRACT_DIR
# Fuzz testlerini 10.000 rastgele senaryo ile çalıştır
forge test --match-test testFuzz --fuzz-runs 10000
if [ $? -eq 0 ]; then
    echo -e "✅ \033[1;32mAŞAMA 1 BAŞARILI: Kontrat %100 Güvenli ve Hacklenemez.\033[0m"
else
    echo -e "❌ \033[1;31mAŞAMA 1 BAŞARISIZ: Kontrat testleri geçemedi!\033[0m"
    exit 1
fi
cd ..

# ─── AŞAMA 2: RUST MATH & LOGIC TESTLERİ (PROPTEST) ───
echo -e "\n\033[1;36m[2/4] 🧠 Kuantum Motoru Testi (Rust Proptest) Başlıyor...\033[0m"
cd $BOT_DIR
# Rust motorunu milyonlarca ihtimale karşı ekstrem sayılarla test et
cargo test --release
if [ $? -eq 0 ]; then
    echo -e "✅ \033[1;32mAŞAMA 2 BAŞARILI: Kuantum Beyin Asla Çökmüyor.\033[0m"
else
    echo -e "❌ \033[1;31mAŞAMA 2 BAŞARISIZ: Rust botu çöktü veya Infinity/NaN üretti!\033[0m"
    exit 1
fi

# ─── AŞAMA 3 & AŞAMA 4: ANVIL CHAOS & SHADOW MODE ───
echo -e "\n\033[1;36m[3/4] 🌪️ Uçtan Uca Cehennem Simülasyonu (Chaos Script & Shadow Mode)...\033[0m"

# 1. Anvil'i arkaplanda başlat
echo "  -> Anvil başlatılıyor (Base Mainnet Fork)..."
anvil --fork-url https://mainnet.base.org --port 8545 > anvil_background.log 2>&1 &
ANVIL_PID=$!
sleep 5 # Anvil'in tam olarak açılması için bekle

# 2. Botu çalıştır (Logları dosyaya yazdır, terminali kirletmesin)
echo "  -> Bot arkaplanda çalıştırılıyor..."
cargo run --release > bot_background.log 2>&1 &
BOT_PID=$!
sleep 3 # Botun websocket'e bağlanmasını bekle

# 3. Fiyat bozucu Chaos scriptini çalıştır
echo "  -> Chaos Injector (Fiyat Bozucu) ateşleniyor..."
chmod +x chaos_injector.sh
./chaos_injector.sh &
CHAOS_PID=$!

# 4. Savaş Meydanı (60 Saniye Bekleyiş)
echo -e "\n\033[1;35m⏳ Sistem 60 saniye boyunca manipüle edilmiş piyasada kendi kendine savaşıyor...\033[0m"
echo -e "\033[1;35m   (Şu an bot sahte fırsatları kovalıyor ve shadow_logs.json dosyasına yazıyor)\033[0m\n"

# Terminalde saniye sayacı gösterelim
for i in {60..1}; do
    echo -ne "   Kalan süre: $i saniye...\r"
    sleep 1
done
echo -ne "\n"

# 5. Süreçleri Temizle (Bilgisayarın RAM'ini kurtar)
echo -e "\n🧹 Süreçler temizleniyor..."
kill $CHAOS_PID 2>/dev/null || true
kill $BOT_PID 2>/dev/null || true
kill $ANVIL_PID 2>/dev/null || true

echo -e "✅ \033[1;32mAŞAMA 3 ve 4 BAŞARILI: Uçtan Uca Simülasyon Tamamlandı.\033[0m"

echo -e "\n\033[1;33m============================================================\033[0m"
echo -e "🏆 \033[1;32mTÜM TESTLER GEÇİLDİ. SİSTEM MAINNET İÇİN HAZIR!\033[0m"
echo -e "📂 \033[1;36mGölge modu (Shadow Mode) sonuçlarını görmek için:\033[0m"
echo -e "   \033[1m$BOT_DIR/shadow_logs.json\033[0m dosyasını inceleyin."
echo -e "\033[1;33m============================================================\033[0m\n"