# Locagens — Yeni Sürüm Yayınlama Rehberi

Bu belge, Locagens masaüstü uygulamasının nasıl yayınlanacağını açıklar.

---

## Yöntem 1: Önerilen (Tag ile Sürüm)

En temiz yol. Hem DMG üretilir hem de GitHub Release otomatik oluşturulur.

1. `apps/desktop/package.json` içindeki `version` alanını güncelleyin
   (örn. `0.1.0` → `0.2.0`).
2. Değişiklikleri commit edin:
   ```bash
   git add apps/desktop/package.json
   git commit -m "bump: v0.2.0"
   ```
3. Tag oluşturun ve push edin:
   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```
4. GitHub Actions otomatik olarak tetiklenir:
   - db-writer (Node.js sidecar), shared paket, web ve API bundle derlenir
   - electron-builder DMG üretir
   - DMG dosyası yeni oluşturulan GitHub Release'e eklenir
   - Sürüm notları otomatik üretilir

İşlem bittiğinde GitHub Releases sayfasından DMG'yi indirebilirsiniz.

---

## Yöntem 2: Manuel (Sadece Build, Release Yok)

Sadece DMG üretmek istiyorsanız (örn. test amaçlı) tag gerekmez:

1. GitHub → Actions → **"Release macOS DMG"** workflow'una gidin.
2. **Run workflow** butonuna tıklayın.
3. `tag_name` alanını boş bırakın (sadece build yapılır, release oluşturulmaz).
4. Build tamamlandıktan sonra **Artifacts** bölümünden DMG'yi indirin
   (14 gün süreyle saklanır).

Alternatif olarak manuel dispatch'te `tag_name` alanına bir değer girerseniz
(örn. `v0.2.1`), workflow hem DMG üretir hem de o isimle bir GitHub Release
oluşturur.

---

## Kod İmzalama

Henüz kod imzalama yapılmıyor. DMG imzasız üretiliyor. Apple Developer sertifikamız
olduğunda repository secrets üzerinden eklenecek. O zamana kadar kullanıcılar
uygulamayı ilk açışta macOS uyarısıyla karşılaşacak (aşağıya bakın).

---

## Otomatik Güncelleme

Henüz otomatik güncelleme desteği yok. Kullanıcılar yeni sürümleri manuel olarak
GitHub Releases sayfasından indirir.

---

## Platform Desteği

Şu an sadece macOS destekleniyor. Windows ve Linux desteği ileride eklenecek.

---

## İmzasız DMG Uyarısı Hakkında

macOS, imzasız uygulamaları açarken uyarı gösterir:

> "Locagens" cannot be opened because the developer cannot be verified.

Bu normaldir. Çözüm:

1. Uygulamaya **sağ tıklayın** (veya Control + tıklayın).
2. **Open** seçeneğini seçin.
3. Açma isteği sorulduğunda **Open** butonuna tıklayın.

Alternatif olarak: **System Settings → Privacy & Security →** en alta inin →
**Open Anyway** butonuna tıklayın.

Bu uyarı sadece ilk açılışta görünür.
