//! Tiny i18n layer: a static key table with an English and a Turkish text per
//! key. Components read the language from `UiStore` (a signal, so they
//! re-render on change) and call `store.t("key")` / `store.tf("key", &[..])`.
//!
//! `{0}`, `{1}`… in a text are replaced by the arguments of `tf`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    #[default]
    En,
    Tr,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Tr];

    /// The language's own name (shown in the selector in every language).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Tr => "Türkçe",
        }
    }
}

/// Text for `key` in `lang`. An unknown key returns the key itself, so a
/// missing entry is visible (and caught by the unit tests).
pub fn t(lang: Lang, key: &'static str) -> &'static str {
    match lookup().get(key) {
        Some((en, tr)) => match lang {
            Lang::En => en,
            Lang::Tr => tr,
        },
        None => key,
    }
}

/// [`t`] with `{0}`, `{1}`… replaced by `args`.
pub fn tf(lang: Lang, key: &'static str, args: &[&str]) -> String {
    let mut text = t(lang, key).to_string();
    for (index, arg) in args.iter().enumerate() {
        text = text.replace(&format!("{{{index}}}"), arg);
    }
    text
}

fn lookup() -> &'static HashMap<&'static str, (&'static str, &'static str)> {
    static MAP: OnceLock<HashMap<&'static str, (&'static str, &'static str)>> = OnceLock::new();
    MAP.get_or_init(|| TABLE.iter().map(|(key, en, tr)| (*key, (*en, *tr))).collect())
}

/// `(key, English, Turkish)`.
const TABLE: &[(&str, &str, &str)] = &[
    // --- settings: accounts / sign-in ---
    ("auth.enter_code", "Enter this code on the sign-in page:", "Bu kodu giriş sayfasına gir:"),
    ("auth.finish_in_browser", "Complete the sign-in in your browser. It returns here automatically.", "Girişi tarayıcında tamamla; bitince otomatik olarak buraya dönülür."),
    ("auth.open_browser", "Open browser", "Tarayıcıyı aç"),
    ("auth.paste_placeholder", "Browser on another machine? Paste the final redirect URL here…", "Tarayıcı başka bir makinede mi? Son yönlendirme adresini buraya yapıştır…"),
    ("auth.submit", "Submit", "Gönder"),
    ("auth.empty", "No accounts yet. Sign in above or run `lcgn auth login` in a terminal (reads ~/.locagens/auth.json with ~/.codex and ~/.pi fallbacks).", "Henüz hesap yok. Yukarıdan giriş yap ya da terminalde `lcgn auth login` çalıştır (~/.locagens/auth.json, yedek olarak ~/.codex ve ~/.pi okunur)."),
    ("auth.signed_in", "Signed in.", "Giriş yapıldı."),
    ("auth.sign_in_failed", "Sign-in failed: {0}", "Giriş başarısız: {0}"),
    ("auth.token_expired", "access token expired (refreshes on next use)", "erişim anahtarının süresi doldu (ilk kullanımda yenilenir)"),
    ("auth.token_valid_for", "access token valid for ~{0} min", "erişim anahtarı ~{0} dk daha geçerli"),
    // --- settings tabs / page ---
    ("settings.title", "Settings", "Ayarlar"),
    ("tab.providers", "Providers", "Sağlayıcılar"),
    ("tab.auth", "Accounts", "Hesaplar"),
    ("tab.permissions", "Permissions", "İzinler"),
    ("tab.memory", "Memory", "Bellek"),
    ("tab.skills", "Skills", "Beceriler"),
    ("tab.mcp", "MCP", "MCP"),
    ("tab.plugins", "Plugins", "Eklentiler"),
    ("tab.security", "Security", "Güvenlik"),
    ("tab.appearance", "Appearance", "Görünüm"),
    // --- common ---
    ("common.delete", "Delete", "Sil"),
    ("common.browse", "Browse…", "Gözat…"),
    ("common.cancel", "Cancel", "Vazgeç"),
    ("common.back", "Back", "Geri"),
    ("common.next", "Next", "İleri"),
    ("common.prev", "Prev", "Önceki"),
    ("common.back_to_chat", "Back to Chat", "Sohbete dön"),
    ("common.provider", "Provider", "Sağlayıcı"),
    ("common.model", "Model", "Model"),
    ("common.save", "Save", "Kaydet"),
    ("common.edit", "Edit", "Düzenle"),
    ("common.enabled", "enabled", "etkin"),
    ("common.disabled", "disabled", "devre dışı"),
    ("common.enable", "Enable", "Etkinleştir"),
    ("common.disable", "Disable", "Devre dışı bırak"),
    // --- window / header / status ---
    ("window.minimize", "Minimize", "Simge durumuna küçült"),
    ("window.maximize", "Maximize", "Büyüt"),
    ("window.restore", "Restore", "Geri yükle"),
    ("window.close", "Close", "Kapat"),
    ("conversation.new", "New Conversation", "Yeni oturum"),
    ("header.stop", "Stop the running turn", "Çalışan turu durdur"),
    ("header.to_light", "Switch to light theme", "Açık temaya geç"),
    ("header.to_dark", "Switch to dark theme", "Koyu temaya geç"),
    ("header.inspector", "Toggle inspector panel", "Denetçi panelini aç/kapat"),
    ("status.created", "Starting", "Başlıyor"),
    ("status.generating", "Running", "Çalışıyor"),
    ("status.awaiting_permission", "Awaiting approval", "Onay bekliyor"),
    ("status.awaiting_input", "Awaiting answer", "Yanıt bekliyor"),
    ("status.done", "Done", "Bitti"),
    ("status.failed", "Failed", "Başarısız"),
    ("status.cancelled", "Cancelled", "İptal edildi"),
    ("status.unknown", "Unknown", "Bilinmiyor"),
    // --- relative time ---
    ("time.now", "now", "şimdi"),
    ("time.minutes", "{0}m", "{0} dk"),
    ("time.hours", "{0}h", "{0} sa"),
    ("time.days", "{0}d", "{0} g"),
    // --- sidebar ---
    ("sidebar.toggle", "Toggle sidebar", "Kenar çubuğunu aç/kapat"),
    ("sidebar.back", "Back", "Geri"),
    ("sidebar.forward", "Forward", "İleri"),
    ("sidebar.search", "Search conversations", "Oturumlarda ara"),
    ("sidebar.usage", "Usage", "Kullanım"),
    ("sidebar.customizations", "Customizations", "Özelleştirmeler"),
    ("sidebar.filter_conversations", "Filter conversations…", "Oturumları filtrele…"),
    ("sidebar.projects", "Projects", "Projeler"),
    ("sidebar.filter_projects", "Filter projects & conversations", "Projeleri ve oturumları filtrele"),
    ("sidebar.add_project", "Add project folder", "Proje klasörü ekle"),
    ("sidebar.project_options", "Project options", "Proje seçenekleri"),
    ("sidebar.new_in_project", "New conversation in project", "Projede yeni oturum"),
    ("sidebar.remove_workspace", "Remove workspace", "Çalışma alanını kaldır"),
    ("sidebar.conversations", "Conversations", "Oturumlar"),
    ("sidebar.new_conversation", "New conversation", "Yeni oturum"),
    ("sidebar.conversation_options", "Conversation options", "Oturum seçenekleri"),
    ("sidebar.fork", "Fork", "Çatalla"),
    ("sidebar.see_all", "See all", "Tümünü gör"),
    ("sidebar.add_project_title", "Add Project", "Proje ekle"),
    ("sidebar.project_folder", "Project folder", "Proje klasörü"),
    ("sidebar.empty", "No conversations yet — start a New Conversation.", "Henüz oturum yok — yeni bir oturum başlat."),
    // --- usage logs ---
    ("usage.title", "Usage Logs", "Kullanım kayıtları"),
    ("usage.search", "Search provider / model / run…", "Sağlayıcı / model / oturum ara…"),
    ("usage.total_cost", "Total cost", "Toplam maliyet"),
    ("usage.model_calls", "Model calls", "Model çağrıları"),
    ("usage.tokens", "Tokens", "Token"),
    ("usage.avg_cache", "Avg cache hit", "Ort. önbellek isabeti"),
    ("usage.when", "When", "Zaman"),
    ("usage.run", "Run", "Oturum"),
    ("usage.role", "Role", "Rol"),
    ("usage.in", "In", "Girdi"),
    ("usage.out", "Out", "Çıktı"),
    ("usage.cache", "Cache", "Önbellek"),
    ("usage.cost", "Cost", "Maliyet"),
    ("usage.empty", "No usage entries yet.", "Henüz kullanım kaydı yok."),
    ("usage.entries", "entries", "kayıt"),
    // --- empty chat / thread ---
    ("empty.title", "What should we build?", "Ne inşa edelim?"),
    ("empty.title_in", "What should we build in {0}?", "{0} içinde ne inşa edelim?"),
    ("empty.subtitle", "Ask the agent to explore, plan, or make changes.", "Ajandan keşfetmesini, plan yapmasını ya da değişiklik yapmasını iste."),
    ("thread.working", "Working…", "Çalışıyor…"),
    ("thread.thought", "Thought · {0} words", "Düşünce · {0} kelime"),
    ("thread.copy", "Copy message", "Mesajı kopyala"),
    ("tool.done", "done", "bitti"),
    ("tool.running", "running", "çalışıyor"),
    ("tool.no_output", "(no output)", "(çıktı yok)"),
    ("act.ran", "Ran {0} {1}", "{0} {1} çalıştırıldı"),
    ("act.edited", "Edited {0} {1}", "{0} {1} düzenlendi"),
    ("act.used", "Used {0} {1}", "{0} {1} kullanıldı"),
    ("act.command", "command", "komut"),
    ("act.commands", "commands", "komut"),
    ("act.file", "file", "dosya"),
    ("act.files", "files", "dosya"),
    ("act.tool", "tool", "araç"),
    ("act.tools", "tools", "araç"),
    ("starter.explore.title", "Explore codebase architecture", "Kod tabanının mimarisini keşfet"),
    ("starter.explore.prompt", "Map the main modules, entry points and how data flows through the project.", "Ana modülleri, giriş noktalarını ve verinin proje içinde nasıl aktığını çıkar."),
    ("starter.review.title", "Review uncommitted git changes", "Commit edilmemiş değişiklikleri incele"),
    ("starter.review.prompt", "Run `git status` / `git diff` and review the pending changes for issues.", "`git status` / `git diff` çalıştır ve bekleyen değişiklikleri sorunlar açısından incele."),
    ("starter.tests.title", "Run and fix failing tests", "Başarısız testleri çalıştır ve düzelt"),
    ("starter.tests.prompt", "Run the test suite, diagnose failures and fix them.", "Test takımını çalıştır, hataların nedenini bul ve düzelt."),
    ("starter.plan.title", "Draft a step-by-step plan", "Adım adım bir plan hazırla"),
    ("starter.plan.prompt", "Turn my task into an ordered, checkable plan before making changes.", "Değişiklik yapmadan önce görevimi sıralı, işaretlenebilir bir plana dönüştür."),
    // --- permission / question cards ---
    ("perm.allow_once", "Allow Once", "Bir kez izin ver"),
    ("perm.allow_run", "Allow for Run", "Bu oturum için izin ver"),
    ("perm.allow_project", "Allow in Project", "Projede izin ver"),
    ("perm.allow_always", "Always Allow", "Her zaman izin ver"),
    ("perm.deny", "Deny", "Reddet"),
    ("perm.network", "network", "ağ"),
    ("perm.requested", "Permission requested", "İzin isteniyor"),
    ("perm.wants_run", "The agent wants to run", "Ajan şunu çalıştırmak istiyor:"),
    ("perm.on", "on", "hedef:"),
    ("question.title", "Agent question", "Ajanın sorusu"),
    ("question.custom", "Or type a custom answer…", "Ya da kendi yanıtını yaz…"),
    ("question.submit", "Submit Answer", "Yanıtı gönder"),
    // --- composer ---
    ("composer.queue", "Queue", "Sıraya al"),
    ("composer.continue", "Continue", "Devam et"),
    ("composer.send", "Send", "Gönder"),
    ("composer.placeholder", "Ask anything, @ to mention, / for commands", "Her şeyi sor, @ ile an, / ile komut ver"),
    ("composer.add_context", "Add Context", "Bağlam ekle"),
    ("composer.files", "Files", "Dosyalar"),
    ("composer.folder", "Folder", "Klasör"),
    ("composer.mentions", "Mentions", "Anmalar"),
    ("composer.actions", "Actions", "Eylemler"),
    ("composer.filter_files", "Filter files…", "Dosyaları filtrele…"),
    ("composer.no_files", "No matching files", "Eşleşen dosya yok"),
    ("composer.no_skills", "No skills found", "Beceri bulunamadı"),
    ("composer.no_providers", "No providers configured", "Yapılandırılmış sağlayıcı yok"),
    ("composer.no_models", "No models", "Model yok"),
    ("composer.mode_title", "Execution mode", "Çalışma modu"),
    ("composer.mode", "Mode", "Mod"),
    ("composer.resolve_first", "Resolve the pending card above first", "Önce yukarıdaki bekleyen kartı yanıtla"),
    ("mode.build", "Build", "Geliştir"),
    ("mode.plan", "Plan", "Plan"),
    ("mode.auto", "Full Access", "Tam erişim"),
    ("effort.minimal", "Minimal", "En düşük"),
    ("effort.low", "Low", "Düşük"),
    ("effort.medium", "Medium", "Orta"),
    ("effort.high", "High", "Yüksek"),
    ("effort.xhigh", "Extra High", "Çok yüksek"),
    ("effort.max", "Max", "En yüksek"),
    // --- inspector ---
    ("inspector.title", "Inspector", "Denetçi"),
    ("inspector.plan", "Plan", "Plan"),
    ("inspector.agents", "Agents", "Ajanlar"),
    ("inspector.git", "Git", "Git"),
    ("inspector.terminal", "Terminal", "Terminal"),
    ("inspector.changes", "Changes", "Değişiklikler"),
    ("inspector.environment", "Environment", "Ortam"),
    ("inspector.context", "Context", "Bağlam"),
    ("inspector.window_used", "Window used", "Kullanılan pencere"),
    ("inspector.no_plan", "No plan yet", "Henüz plan yok"),
    ("inspector.no_plan_hint", "Switch to Plan mode and ask for one.", "Plan moduna geç ve bir plan iste."),
    ("inspector.no_agents", "No sub-agents yet", "Henüz alt ajan yok"),
    ("inspector.no_agents_hint", "Agents the model spawns will show up here.", "Modelin başlattığı ajanlar burada görünür."),
    ("git.turn_diff", "Active Turn Diff ({0} files)", "Etkin tur farkı ({0} dosya)"),
    ("git.undo_title", "Restore the project to the pre-turn snapshot (git)", "Projeyi tur öncesi anlık görüntüye geri yükle (git)"),
    ("git.undo", "Undo", "Geri al"),
    ("git.stage_all", "Stage all", "Tümünü stage et"),
    ("git.unstage_all", "Unstage all", "Tümünü stage'den çıkar"),
    ("git.stage", "Stage", "Stage et"),
    ("git.unstage", "Unstage", "Stage'den çıkar"),
    ("git.dirty", "Uncommitted changes", "Commit edilmemiş değişiklikler"),
    ("git.clean", "Clean", "Temiz"),
    ("git.changed_files", "Changed files", "Değişen dosyalar"),
    ("git.worktrees", "Worktrees", "Çalışma ağaçları"),
    ("git.branch", "Branch…", "Dal…"),
    ("git.worktree", "Worktree", "Çalışma ağacı"),
    ("git.no_worktrees", "No worktrees.", "Çalışma ağacı yok."),
    ("git.detached", "(detached)", "(ayrık)"),
    ("git.remove_worktree", "Remove worktree", "Çalışma ağacını kaldır"),
    ("git.commit_message", "Commit message…", "Commit mesajı…"),
    ("git.commit", "Commit", "Commit"),
    ("git.push", "Push", "Push"),
    ("git.not_repo", "Not a git repository", "Git deposu değil"),
    ("git.not_repo_hint", "The selected folder has no git history.", "Seçili klasörde git geçmişi yok."),
    ("git.no_project", "No project selected", "Proje seçilmedi"),
    ("git.no_project_hint", "Select a project to inspect its changes.", "Değişiklikleri görmek için bir proje seç."),
    ("git.diff", "Diff", "Fark"),
    ("term.command", "Command to run in the background…", "Arka planda çalıştırılacak komut…"),
    ("term.run", "Run", "Çalıştır"),
    ("term.none", "No processes", "İşlem yok"),
    ("term.none_hint", "Run a command above to start one in the background.", "Arka planda başlatmak için yukarıda bir komut çalıştır."),
    ("term.stop", "Stop process", "İşlemi durdur"),
    ("term.no_output", "No output yet…", "Henüz çıktı yok…"),
    ("term.logs", "Logs", "Günlük"),
    // --- settings tabs content ---
    ("providers.none", "No providers configured. Add one via `lcgn auth login` or the provider config file (~/.locagens/auth.json, Codex-compatible with ~/.codex/auth.json).", "Yapılandırılmış sağlayıcı yok. `lcgn auth login` ile ya da sağlayıcı yapılandırma dosyasıyla ekle (~/.locagens/auth.json, ~/.codex/auth.json ile Codex uyumlu)."),
    ("providers.default", "default", "varsayılan"),
    ("providers.models", "models", "model"),
    ("providers.use", "Use", "Kullan"),
    ("perms.none", "No standing permission grants yet. Approve with \"Always Allow\" / \"Allow in Project\" to create them.", "Henüz kalıcı izin yok. \"Her zaman izin ver\" / \"Projede izin ver\" ile onaylayarak oluştur."),
    ("memory.placeholder", "Remember… (a fact the agent should always know)", "Hatırla… (ajanın her zaman bilmesi gereken bir bilgi)"),
    ("memory.none", "No memories stored yet.", "Henüz kayıtlı bellek yok."),
    ("skills.no_project", "no project selected", "proje seçilmedi"),
    ("skills.new", "New skill", "Yeni beceri"),
    ("skills.edit", "Edit skill", "Beceriyi düzenle"),
    ("skills.new_md", "New skill (SKILL.md)", "Yeni beceri (SKILL.md)"),
    ("skills.user_scope", "user scope", "kullanıcı kapsamı"),
    ("skills.project_scope", "project scope", "proje kapsamı"),
    ("skills.need_project", "Select a project for project-scope skills.", "Proje kapsamlı beceriler için bir proje seç."),
    ("skills.none", "No skills found in the user or project skills directories.", "Kullanıcı ya da proje beceri klasörlerinde beceri bulunamadı."),
    ("mcp.servers", "MCP servers", "MCP sunucuları"),
    ("mcp.add", "Add server", "Sunucu ekle"),
    ("mcp.name", "name (lowercase-dash)", "ad (küçük-harf-tire)"),
    ("mcp.command", "command, e.g. npx some-mcp-server", "komut, örn. npx some-mcp-server"),
    ("mcp.url", "https://… (SSE endpoint)", "https://… (SSE uç noktası)"),
    ("mcp.save", "Save server", "Sunucuyu kaydet"),
    ("mcp.none", "No MCP servers configured. Add one above or edit mcp.json directly.", "Yapılandırılmış MCP sunucusu yok. Yukarıdan ekle ya da mcp.json dosyasını doğrudan düzenle."),
    ("mcp.tools", "tools", "araç"),
    ("plugins.install", "Install", "Kur"),
    ("plugins.choose", "Choose a template…", "Bir şablon seç…"),
    ("plugins.install_template", "Install template", "Şablonu kur"),
    ("plugins.local_path", "Local plugin folder (absolute path)…", "Yerel eklenti klasörü (tam yol)…"),
    ("plugins.install_local", "Install local", "Yerel kur"),
    ("plugins.none", "No plugins installed. Install a template above.", "Kurulu eklenti yok. Yukarıdan bir şablon kur."),
    ("plugins.uninstall", "Uninstall", "Kaldır"),
    ("security.sandbox", "Command sandbox", "Komut korumalı alanı"),
    ("security.platform", "Platform", "Platform"),
    ("security.status", "Status", "Durum"),
    ("security.policy", "Workspace policy", "Çalışma alanı ilkesi"),
    ("security.network", "Command network", "Komut ağ erişimi"),
    ("security.writes_only", "Workspace writes only", "Yalnızca çalışma alanına yazma"),
    ("security.bind", "API bind address", "API bağlama adresi"),
    ("security.authenticated", "authenticated", "kimlik doğrulamalı"),
    ("security.setup", "Run Windows sandbox setup", "Windows korumalı alan kurulumunu çalıştır"),
    ("security.unavailable", "Security status unavailable.", "Güvenlik durumu alınamadı."),
    // --- appearance tab ---
    ("appearance.title", "Appearance", "Görünüm"),
    ("appearance.language", "Language", "Dil"),
    ("appearance.language_desc", "Language of the app interface.", "Uygulama arayüzünün dili."),
    ("appearance.theme", "Theme", "Tema"),
    ("appearance.theme_desc", "System follows your operating system's light or dark setting.", "Sistem, işletim sisteminin açık ya da koyu ayarını izler."),
    ("appearance.theme.dark", "Dark", "Koyu"),
    ("appearance.theme.light", "Light", "Açık"),
    ("appearance.theme.system", "System", "Sistem"),
    ("appearance.ui_font", "Interface font", "Arayüz yazı tipi"),
    ("appearance.ui_font_desc", "Used for menus, buttons and chat text. Fonts that are not installed fall back to the system font.", "Menüler, düğmeler ve sohbet metni için. Kurulu olmayan yazı tipleri sistem yazı tipine döner."),
    ("appearance.code_font", "Code font", "Kod yazı tipi"),
    ("appearance.code_font_desc", "Used for code blocks, diffs, commands and tool output.", "Kod blokları, farklar, komutlar ve araç çıktıları için."),
    ("appearance.font.system", "System", "Sistem"),
    ("appearance.font.custom", "Custom", "Özel"),
    ("appearance.font_placeholder", "Font family name", "Yazı tipi ailesi adı"),
    ("appearance.ligatures", "Code ligatures", "Kod ligatürleri"),
    ("appearance.ligatures_desc", "Join sequences like => and != into single glyphs in code.", "Kodda => ve != gibi dizileri tek karakter olarak göster."),
    ("appearance.on", "On", "Açık"),
    ("appearance.off", "Off", "Kapalı"),
    ("appearance.size", "Font size", "Yazı boyutu"),
    ("appearance.size_desc", "Scales all text in the app.", "Uygulamadaki tüm metni ölçekler."),
    ("appearance.size.small", "Small", "Küçük"),
    ("appearance.size.medium", "Medium", "Orta"),
    ("appearance.size.large", "Large", "Büyük"),
    ("appearance.preview_ui", "The quick brown fox jumps over the lazy dog 0123456789", "Pijamalı hasta yağız şoföre çabucak güvendi 0123456789"),
    ("appearance.preview_code", "fn main() { let ok = x != y && a => b; }", "fn main() { let tamam = x != y && a => b; }"),
    // --- toasts ---
    ("toast.compacted", "Context compacted ({0} → {1} messages)", "Bağlam sıkıştırıldı ({0} → {1} ileti)"),
    ("toast.run_failed", "Run failed: {0}", "Oturum başarısız oldu: {0}"),
    ("toast.decision_failed", "Failed to apply decision: {0}", "Karar uygulanamadı: {0}"),
    ("toast.answer_failed", "Failed to answer: {0}", "Yanıt gönderilemedi: {0}"),
    ("toast.stop_failed", "Failed to stop run: {0}", "Oturum durdurulamadı: {0}"),
    ("toast.add_project_failed", "Failed to add project: {0}", "Proje eklenemedi: {0}"),
    ("toast.remove_project_failed", "Failed to remove project: {0}", "Proje kaldırılamadı: {0}"),
    ("toast.forked", "Conversation forked", "Oturum çatallandı"),
    ("toast.fork_failed", "Fork failed: {0}", "Çatallama başarısız: {0}"),
    ("toast.no_fork", "No active conversation to fork.", "Çatallanacak etkin oturum yok."),
    ("toast.undo_failed", "Undo failed: {0}", "Geri alma başarısız: {0}"),
    ("toast.restored", "Restored snapshot {0}", "{0} anlık görüntüsü geri yüklendi"),
    ("toast.select_project", "Select a project first.", "Önce bir proje seç."),
    ("toast.mode_set", "Mode set to {0}", "Mod {0} olarak ayarlandı"),
    ("toast.unknown_command", "Unknown command {0} — try /new /fork /undo /plan /build /auto /chat", "Bilinmeyen komut {0} — şunları dene: /new /fork /undo /plan /build /auto /chat"),
    ("toast.resolve_first", "Resolve the pending permission or question first.", "Önce bekleyen izni ya da soruyu yanıtla."),
    ("toast.select_provider", "Select a provider and model first (Settings → Providers).", "Önce bir sağlayıcı ve model seç (Ayarlar → Sağlayıcılar)."),
    ("toast.queue_failed", "Failed to queue message: {0}", "Mesaj sıraya alınamadı: {0}"),
    ("toast.continue_failed", "Failed to continue run: {0}", "Oturum sürdürülemedi: {0}"),
    ("toast.start_failed", "Failed to start run: {0}", "Oturum başlatılamadı: {0}"),
    ("toast.stage_failed", "Stage failed: {0}", "Stage işlemi başarısız: {0}"),
    ("toast.unstage_failed", "Unstage failed: {0}", "Stage'den çıkarma başarısız: {0}"),
    ("toast.worktree_created", "Worktree created ({0})", "Çalışma ağacı oluşturuldu ({0})"),
    ("toast.worktree_failed", "Worktree failed: {0}", "Çalışma ağacı hatası: {0}"),
    ("toast.switch_failed", "Switch failed: {0}", "Geçiş başarısız: {0}"),
    ("toast.worktree_removed", "Worktree removed", "Çalışma ağacı kaldırıldı"),
    ("toast.remove_failed", "Remove failed: {0}", "Kaldırma başarısız: {0}"),
    ("toast.commit_failed", "Commit failed: {0}", "Commit başarısız: {0}"),
    ("toast.committed", "Committed.", "Commit edildi."),
    ("toast.push_failed", "Push failed: {0}", "Push başarısız: {0}"),
    ("toast.pushed", "Pushed.", "Push edildi."),
    ("toast.proc_start_failed", "Failed to start: {0}", "Başlatılamadı: {0}"),
    ("toast.proc_stopped", "Process stopped.", "İşlem durduruldu."),
    ("toast.stop_proc_failed", "Stop failed: {0}", "Durdurma başarısız: {0}"),
    ("toast.delete_rule_failed", "Failed to delete rule: {0}", "Kural silinemedi: {0}"),
    ("toast.save_memory_failed", "Failed to save memory: {0}", "Bellek kaydedilemedi: {0}"),
    ("toast.update_memory_failed", "Failed to update memory: {0}", "Bellek güncellenemedi: {0}"),
    ("toast.delete_memory_failed", "Failed to delete memory: {0}", "Bellek silinemedi: {0}"),
    ("toast.skill_saved", "Skill \"{0}\" saved.", "\"{0}\" becerisi kaydedildi."),
    ("toast.save_skill_failed", "Failed to save skill: {0}", "Beceri kaydedilemedi: {0}"),
    ("toast.delete_skill_failed", "Failed to delete skill: {0}", "Beceri silinemedi: {0}"),
    ("toast.toggle_failed", "Toggle failed: {0}", "Değiştirme başarısız: {0}"),
    ("toast.delete_failed", "Delete failed: {0}", "Silme başarısız: {0}"),
    ("toast.installed", "Installed \"{0}\".", "\"{0}\" kuruldu."),
    ("toast.install_failed", "Install failed: {0}", "Kurulum başarısız: {0}"),
    ("toast.uninstall_failed", "Uninstall failed: {0}", "Kaldırma başarısız: {0}"),
    ("toast.sandbox_failed", "Sandbox setup failed: {0}", "Korumalı alan kurulumu başarısız: {0}"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn placeholders(text: &str) -> Vec<usize> {
        (0..4).filter(|i| text.contains(&format!("{{{i}}}"))).collect()
    }

    #[test]
    fn every_key_has_text_in_both_languages() {
        let mut seen = HashSet::new();
        for (key, en, tr) in TABLE {
            assert!(seen.insert(*key), "duplicate key {key}");
            assert!(!en.trim().is_empty(), "{key}: empty English text");
            assert!(!tr.trim().is_empty(), "{key}: empty Turkish text");
            assert_eq!(placeholders(en), placeholders(tr), "{key}: placeholders differ between EN and TR");
        }
    }

    #[test]
    fn lookup_returns_the_right_language() {
        assert_eq!(t(Lang::En, "settings.title"), "Settings");
        assert_eq!(t(Lang::Tr, "settings.title"), "Ayarlar");
        assert_eq!(t(Lang::Tr, "conversation.new"), "Yeni oturum");
        assert_eq!(t(Lang::Tr, "composer.send"), "Gönder");
        assert_eq!(t(Lang::En, "no.such.key"), "no.such.key");
        assert_eq!(tf(Lang::Tr, "empty.title_in", &["foo"]), "foo içinde ne inşa edelim?");
        assert_eq!(tf(Lang::En, "act.ran", &["3", "commands"]), "Ran 3 commands");
    }

    /// Every `"namespace.key"` literal in the UI sources must exist in the
    /// table, so no component can reference a key that lacks EN/TR text.
    #[test]
    fn every_key_used_in_the_sources_exists() {
        let namespaces: HashSet<&str> = TABLE.iter().filter_map(|(key, _, _)| key.split('.').next()).collect();
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs(&src, &mut files);
        let known: HashSet<&str> = TABLE.iter().map(|(key, _, _)| *key).collect();
        let mut missing = Vec::new();
        for file in files {
            if file.file_name().is_some_and(|name| name == "i18n.rs" || name == "prefs.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            let text = text.replace("\\\"", "\"");
            for literal in text.split('"').skip(1).step_by(2) {
                let mut parts = literal.splitn(2, '.');
                let (Some(ns), Some(rest)) = (parts.next(), parts.next()) else { continue };
                let looks_like_key = namespaces.contains(ns)
                    && !rest.is_empty()
                    && literal.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_');
                if looks_like_key && !known.contains(literal) {
                    missing.push(format!("{}: {literal}", file.display()));
                }
            }
        }
        assert!(missing.is_empty(), "keys without EN/TR text: {missing:#?}");
    }

    fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
}
