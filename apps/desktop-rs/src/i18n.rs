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

static CURRENT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Records the UI language for code that has no `UiStore` (markdown HTML).
pub fn set_current(lang: Lang) {
    CURRENT.store(lang as u8, std::sync::atomic::Ordering::Relaxed);
}

/// [`t`] in the language last recorded with [`set_current`].
pub fn tc(key: &'static str) -> &'static str {
    let lang = if CURRENT.load(std::sync::atomic::Ordering::Relaxed) == 1 { Lang::Tr } else { Lang::En };
    t(lang, key)
}

/// i18n key of a sub-agent role label (`explorer`, `worker`, else the main agent).
pub fn role_key(role: &str) -> &'static str {
    match role {
        "explorer" => "agent.explorer",
        "worker" => "agent.worker",
        _ => "agent.main",
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

/// Translated description of a slash command (`slash.<name>`), falling back
/// to the shared English description.
pub fn command_description(lang: Lang, spec: &locagens_core::commands::CommandSpec) -> &'static str {
    match lookup().get(format!("slash.{}", spec.name).as_str()) {
        Some((en, tr)) => match lang {
            Lang::En => en,
            Lang::Tr => tr,
        },
        None => spec.description,
    }
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
    ("header.inspector", "Toggle inspector panel ({0})", "Denetçi panelini aç/kapat ({0})"),
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
    ("sidebar.toggle", "Hide sidebar ({0})", "Kenar çubuğunu gizle ({0})"),
    ("sidebar.show", "Show sidebar ({0})", "Kenar çubuğunu göster ({0})"),
    ("sidebar.new_chat_title", "New chat ({0})", "Yeni oturum ({0})"),
    ("sidebar.back", "Back", "Geri"),
    ("sidebar.forward", "Forward", "İleri"),
    ("sidebar.search", "Search conversations", "Oturumlarda ara"),
    ("sidebar.usage", "Usage", "Kullanım"),
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
    ("thread.writing_patch", "Writing changes", "Değişiklikler yazılıyor"),
    ("thread.thought", "Thought · {0} words", "Düşünce · {0} kelime"),
    ("thread.copy", "Copy message", "Mesajı kopyala"),
    ("thread.stopped", "You stopped this turn; the answer above may be incomplete.", "Bu turu durdurdun; yukarıdaki yanıt eksik olabilir."),
    ("tool.done", "done", "bitti"),
    ("tool.running", "running", "çalışıyor"),
    ("tool.awaiting_approval", "waiting for approval", "onay bekliyor"),
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
    // --- project trust banner ---
    ("trust.title", "Untrusted project", "Güvenilmeyen proje"),
    ("trust.body", "This folder ships configuration that can run code on your machine (hooks, MCP servers, plugins, rules or agent roles). It is ignored until you trust the folder.", "Bu klasör makinende kod çalıştırabilecek yapılandırma içeriyor (hook'lar, MCP sunucuları, eklentiler, kurallar veya ajan rolleri). Klasöre güvenene kadar yok sayılır."),
    ("trust.trust", "Trust folder", "Klasöre güven"),
    ("trust.not_now", "Not now", "Şimdi değil"),
    ("trust.done", "Folder trusted: its project configuration loads from the next run.", "Klasöre güvenildi: proje yapılandırması bir sonraki çalıştırmadan itibaren yüklenir."),
    ("trust.failed", "Could not trust the folder: {0}", "Klasöre güvenilemedi: {0}"),
    ("perm.allow_once", "Allow once", "Bir kez izin ver"),
    ("perm.allow_run", "For this run", "Bu tur boyunca"),
    ("perm.allow_project", "In this project", "Bu projede"),
    ("perm.allow_always", "Everywhere", "Her yerde"),
    ("perm.deny", "Deny", "Reddet"),
    ("perm.network", "network", "ağ"),
    ("perm.network_best_effort", "best-effort on Windows: programs can bypass the network proxy", "Windows üzerinde en iyi çaba: programlar ağ proxy'sini atlayabilir"),
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
    ("composer.mode_title", "Execution mode ({0})", "Çalışma modu ({0})"),
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
    ("providers.models", "models", "model"),
    ("providers.subtitle", "Providers you are signed in to and their models. Pick a model in the composer; use the switches to choose which models it offers.", "Giriş yaptığın sağlayıcılar ve modelleri. Modeli mesaj kutusundan seç; anahtarlarla hangi modellerin listeleneceğini belirle."),
    ("composer.usage_title", "Tokens and cost of this conversation", "Bu konuşmanın token ve maliyeti"),
    ("resize.sidebar", "Resize sidebar", "Kenar çubuğunu yeniden boyutlandır"),
    ("resize.panel", "Resize side panel", "Yan paneli yeniden boyutlandır"),
    ("providers.show_model", "Show in model picker", "Model seçicide göster"),
    ("toast.model_visibility_failed", "Could not save model visibility", "Model görünürlüğü kaydedilemedi"),
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
    ("toast.select_project", "Select a project first.", "Önce bir proje seç."),
    ("toast.unknown_command", "Unknown command {0} — try {1}", "Bilinmeyen komut {0} — şunları dene: {1}"),
    ("toast.command_busy", "{0} is unavailable while a run is generating — wait or stop it first.", "{0} bir oturum çalışırken kullanılamaz — bitmesini bekle ya da durdur."),
    ("toast.command_cli_only", "{0} is only available in the lcgn CLI.", "{0} yalnızca lcgn CLI'da kullanılabilir."),
    ("composer.commands", "Commands", "Komutlar"),
    // Slash-command descriptions, keyed `slash.<name>`; the English text is
    // the shared `locagens_core::commands` description (checked by a test).
    ("slash.plan", "switch to plan mode (read-only)", "plan moduna geç (salt okunur)"),
    ("slash.build", "switch to build mode (edits applied)", "build moduna geç (düzenlemeler uygulanır)"),
    ("slash.auto", "switch to full access (no permission prompts)", "tam erişime geç (izin sorulmaz)"),
    ("slash.chat", "switch to chat mode (no tools)", "sohbet moduna geç (araç yok)"),
    ("slash.new", "start a fresh session on the next message", "sonraki mesajla yeni bir oturum başlat"),
    ("slash.fork", "fork the current session", "geçerli oturumu çatalla"),
    ("slash.undo", "undo the last turn (git snapshot)", "son turu geri al (git anlık görüntüsü)"),
    ("slash.review", "code review: changes, a branch or a commit", "kod incelemesi: değişiklikler, bir dal ya da bir commit"),
    ("slash.help", "show the available commands", "kullanılabilir komutları göster"),
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
    // --- desktop QA fixes (QA-052/053): review, MCP sign-in, badges, markdown ---
    ("review.title", "Code review", "Kod incelemesi"),
    ("review.writing", "Writing the review…", "İnceleme yazılıyor…"),
    ("review.none", "No findings.", "Bulgu yok."),
    ("review.button", "Review uncommitted changes", "Commit edilmemiş değişiklikleri incele"),
    ("review.failed", "Review failed: {0}", "İnceleme başarısız: {0}"),
    ("mcp.signin_opening", "Opening the browser to sign in…", "Giriş için tarayıcı açılıyor…"),
    ("mcp.signin_failed", "MCP sign-in failed: {0}", "MCP girişi başarısız: {0}"),
    ("mcp.sign_in", "Sign in", "Giriş yap"),
    ("mcp.sign_out", "Sign out", "Çıkış yap"),
    ("mcp.transport_stdio", "stdio (local process)", "stdio (yerel süreç)"),
    ("mcp.transport_sse", "SSE (server-sent events)", "SSE (sunucu olayları)"),
    ("mcp.transport_http", "Streamable HTTP", "Streamable HTTP"),
    ("plugin.fallback_name", "plugin", "eklenti"),
    ("agent.explorer", "explorer", "gezgin"),
    ("agent.worker", "worker", "işçi"),
    ("agent.main", "main", "ana"),
    ("thread.file_deleted", "{0} (deleted)", "{0} (silindi)"),
    ("header.tokens_cost", "{0} tok · {1}", "{0} token · {1}"),
    ("header.tokens", "{0} tok", "{0} token"),
    ("inspector.plan_version", "{0} · v{1}", "{0} · sürüm {1}"),
    ("git.badge_created", "A", "E"),
    ("git.badge_deleted", "D", "S"),
    ("git.badge_modified", "M", "D"),
    ("git.kind_created", "Added", "Eklendi"),
    ("git.kind_deleted", "Deleted", "Silindi"),
    ("git.kind_modified", "Modified", "Değiştirildi"),
    ("md.copy", "Copy", "Kopyala"),
    ("md.file_deleted", "+ (file deleted)", "+ (dosya silindi)"),
    // --- desktop production pass: toasts, a11y, common ---
    ("toast.background_waiting", "{0} is waiting for you", "{0} seni bekliyor"),
    ("toast.queued_restored", "The queued message was not sent; it is back in the composer.", "Sıradaki mesaj gönderilmedi; yeniden yazma alanında."),
    ("toast.unknown_model", "Unknown model: {0}", "Bilinmeyen model: {0}"),
    ("toast.unknown_effort", "Unknown thinking level. Available: {0}", "Bilinmeyen düşünme düzeyi. Seçenekler: {0}"),
    ("toast.no_effort", "this model has no thinking levels", "bu modelin düşünme düzeyi yok"),
    ("toast.copy_failed", "Could not copy to the clipboard", "Panoya kopyalanamadı"),
    ("a11y.permission_needed", "Permission needed. Allow once is focused.", "İzin gerekiyor. Bir kez izin ver odakta."),
    ("a11y.question_needed", "The agent asked a question.", "Ajan bir soru sordu."),
    ("common.dismiss", "Dismiss", "Kapat"),
    ("common.confirm_again", "Click again to confirm", "Onaylamak için tekrar tıkla"),
    ("common.close", "Close", "Kapat"),
    ("common.yes", "yes", "evet"),
    ("common.no", "no", "hayır"),
    ("startup.failed", "Locagens could not start:\n\n{0}", "Locagens başlatılamadı:\n\n{0}"),
    // --- permission cards ---
    ("perm.title_command", "Run a command?", "Komut çalıştırılsın mı?"),
    ("perm.title_network", "Connect to {0}?", "{0} adresine bağlanılsın mı?"),
    ("perm.title_edit_files", "Edit files?", "Dosyalar düzenlensin mi?"),
    ("perm.title_edit", "Edit {0}?", "{0} düzenlensin mi?"),
    ("perm.title_tool", "Use {0}?", "{0} kullanılsın mı?"),
    ("perm.asked_by", "Asked by {0} {1}", "Soran: {0} {1}"),
    ("perm.always_menu", "Always allow", "Hep izin ver"),
    ("perm.keys", "Enter allows once · Esc denies", "Enter bir kez izin verir · Esc reddeder"),
    ("perm.reason_protected_path", "This touches a protected path (.git, .locagens, .codex, .agents), so it always asks.", "Korumalı bir yola (.git, .locagens, .codex, .agents) dokunuyor; bu yüzden her zaman sorulur."),
    ("perm.reason_dangerous_command", "This command can destroy data, so it always asks.", "Bu komut veri silebilir; bu yüzden her zaman sorulur."),
    ("perm.reason_network", "This reaches the network.", "Bu işlem ağa erişiyor."),
    ("perm.reason_mode", "The current mode asks before this kind of action.", "Geçerli mod bu tür işlemlerden önce sorar."),
    // --- modes ---
    ("mode.chat", "Chat", "Sohbet"),
    ("mode.build_desc", "Edits files; asks before commands", "Dosyaları düzenler; komutlardan önce sorar"),
    ("mode.plan_desc", "Reads only; writes a plan", "Yalnızca okur; plan yazar"),
    ("mode.auto_desc", "No prompts: commands and network run unasked", "Soru yok: komutlar ve ağ sormadan çalışır"),
    // --- composer / queue / picker ---
    ("composer.connect_first", "Connect a model to start", "Başlamak için bir model bağla"),
    ("composer.folder_first", "Open a project folder to start", "Başlamak için bir proje klasörü aç"),
    ("queue.title", "Queued for the next turn", "Sonraki tur için sırada"),
    ("queue.remove", "Take back into the composer", "Yazma alanına geri al"),
    ("composer.remove_image", "Remove image", "Görseli kaldır"),
    ("composer.image_pasted", "Pasted image", "Yapıştırılan görsel"),
    ("toast.image_too_many", "At most {0} images per message.", "Bir mesajda en fazla {0} görsel olabilir."),
    ("toast.image_too_large", "That image is too large (5 MB limit).", "Bu görsel çok büyük (5 MB sınırı)."),
    ("toast.image_while_generating", "Images cannot be queued while the model is working; send it when the turn ends.", "Model çalışırken görsel kuyruğa alınamaz; tur bitince gönder."),
    ("queue.already_sent", "That message was already sent", "Bu mesaj zaten gönderildi"),
    ("picker.title", "Model ({0})", "Model ({0})"),
    ("picker.search", "Search models", "Model ara"),
    ("picker.effort", "Thinking", "Düşünme"),
    ("picker.manage", "Manage models…", "Modelleri yönet…"),
    // --- palette / shortcuts ---
    ("palette.title", "Command palette", "Komut paleti"),
    ("palette.search", "Search chats, commands and settings", "Oturum, komut ve ayar ara"),
    ("palette.search_chats", "Search chats", "Oturum ara"),
    ("palette.none", "No matches", "Eşleşme yok"),
    ("palette.chat", "Chat", "Oturum"),
    ("palette.command", "Command", "Komut"),
    ("palette.settings", "Settings", "Ayarlar"),
    ("keys.title", "Keyboard shortcuts", "Klavye kısayolları"),
    ("keys.escape", "Stop the turn or close a menu", "Turu durdur ya da menüyü kapat"),
    ("keys.new_chat", "New chat", "Yeni oturum"),
    ("keys.palette", "Command palette", "Komut paleti"),
    ("keys.focus", "Focus the composer", "Yazma alanına odaklan"),
    ("keys.model", "Choose a model", "Model seç"),
    ("keys.sidebar", "Show or hide the sidebar", "Kenar çubuğunu göster/gizle"),
    ("keys.inspector", "Show or hide the inspector", "Denetçiyi göster/gizle"),
    ("keys.mode", "Switch Build / Plan", "Geliştir / Plan arasında geç"),
    ("keys.settings", "Settings", "Ayarlar"),
    ("keys.help", "This list", "Bu liste"),
    ("keys.recall", "Recall the last prompt (empty composer)", "Son mesajı geri getir (boş yazma alanında)"),
    // --- thread: changes, status, failures ---
    ("changes.title", "{0} files changed", "{0} dosya değişti"),
    ("changes.review", "Review", "İncele"),
    ("changes.undo", "Undo", "Geri al"),
    ("changes.confirm", "Revert the {0} files this turn changed?", "Bu turun değiştirdiği {0} dosya geri alınsın mı?"),
    ("changes.undo_confirm", "Revert", "Geri al"),
    ("changes.reverted", "Reverted {0} files", "{0} dosya geri alındı"),
    ("changes.left_alone", "{0} changed since, left alone: {1}", "{0} dosya sonradan değişti, dokunulmadı: {1}"),
    ("undo.reverted", "Reverted {0} files", "{0} dosya geri alındı"),
    ("undo.left_alone", " · left alone (changed since): {0}", " · dokunulmadı (sonradan değişti): {0}"),
    ("status_row.approval", "Waiting for your approval", "Onayın bekleniyor"),
    ("status_row.answer", "Waiting for your answer", "Yanıtın bekleniyor"),
    ("status_row.writing", "Writing {0}", "Yazılıyor: {0}"),
    ("status_row.running_command", "Running {0}", "Çalışıyor: {0}"),
    ("status_row.thinking", "Thinking", "Düşünüyor"),
    ("status_row.esc", "Esc to stop", "Durdurmak için Esc"),
    ("failure.title", "The turn failed", "Tur başarısız oldu"),
    ("failure.no_reason", "No reason was reported.", "Bir neden bildirilmedi."),
    ("failure.copy", "Copy error", "Hatayı kopyala"),
    ("failure.copied", "Copied", "Kopyalandı"),
    ("failure.open_accounts", "Open Models & accounts", "Modeller ve hesaplar'ı aç"),
    ("failure.hint", "Earlier messages are kept. Send again to retry.", "Önceki mesajlar korunur. Yeniden denemek için tekrar gönder."),
    ("tool.failed", "Failed", "Başarısız"),
    ("tool.denied", "Denied", "Reddedildi"),
    ("act.failed", "{0} failed", "{0} başarısız"),
    ("thread.reuse", "Edit", "Düzenle"),
    ("thread.reuse_title", "Put this message back into the composer", "Bu mesajı yazma alanına geri koy"),
    ("verb.ran", "Ran", "Çalıştırıldı"),
    ("verb.sent_input", "Sent input to", "Girdi gönderildi"),
    ("verb.read", "Read", "Okundu"),
    ("verb.listed", "Listed", "Listelendi"),
    ("verb.searched", "Searched", "Arandı"),
    ("verb.wrote", "Wrote", "Yazıldı"),
    ("verb.edited", "Edited", "Düzenlendi"),
    ("verb.applied_patch", "Applied patch", "Yama uygulandı"),
    ("verb.deleted", "Deleted", "Silindi"),
    ("verb.created_dir", "Created dir", "Klasör oluşturuldu"),
    ("verb.moved", "Moved", "Taşındı"),
    ("verb.fetched", "Fetched", "Getirildi"),
    ("verb.spawned_agents", "Spawned agents", "Ajanlar başlatıldı"),
    ("verb.spawned_agent", "Spawned agent", "Ajan başlatıldı"),
    ("verb.waited_agents", "Waited for agents", "Ajanlar beklendi"),
    ("verb.loaded_skill", "Loaded skill", "Beceri yüklendi"),
    ("verb.asked", "Asked", "Soruldu"),
    ("verb.titled_chat", "Titled chat", "Oturum adlandırıldı"),
    ("verb.updated_plan", "Updated plan", "Plan güncellendi"),
    ("verb.remembered", "Remembered", "Hatırlandı"),
    // --- empty thread ---
    ("empty.connect_title", "Connect a model", "Bir model bağla"),
    ("empty.connect_body", "Sign in or add an API key to start working.", "Çalışmaya başlamak için giriş yap ya da API anahtarı ekle."),
    ("empty.connect_button", "Open Models & accounts", "Modeller ve hesaplar'ı aç"),
    ("empty.connect_hint", "Keys stay in ~/.locagens/auth.json, never in the project.", "Anahtarlar projede değil, ~/.locagens/auth.json içinde kalır."),
    ("empty.folder_title", "Open a project folder", "Bir proje klasörü aç"),
    ("empty.folder_body", "The agent works inside the folder you pick.", "Ajan seçtiğin klasörün içinde çalışır."),
    ("empty.folder_button", "Open folder…", "Klasör aç…"),
    ("empty.continue", "Continue: {0} · {1}", "Devam et: {0} · {1}"),
    ("starter.review_changes", "Review the uncommitted changes", "Commit edilmemiş değişiklikleri incele"),
    // --- inspector ---
    ("agents.finished", "finished", "bitti"),
    ("agents.idle", "idle", "boşta"),
    ("agents.working", "working", "çalışıyor"),
    ("git.main", "main tree", "ana ağaç"),
    ("term.status_running", "running", "çalışıyor"),
    ("term.status_exited", "exited", "çıktı"),
    ("term.status_stopped", "stopped", "durduruldu"),
    ("term.status_error", "error", "hata"),
    ("security.status_ready", "ready", "hazır"),
    ("security.status_unavailable", "unavailable", "kullanılamıyor"),
    ("security.status_setup", "needs setup", "kurulum gerekli"),
    ("security.status_degraded", "limited", "sınırlı"),
    // --- settings: models & accounts ---
    ("providers.connect", "Connect", "Bağlan"),
    ("providers.connect_hint", "Sign in with a subscription or add an API key. Keys are stored in ~/.locagens/auth.json.", "Bir abonelikle giriş yap ya da API anahtarı ekle. Anahtarlar ~/.locagens/auth.json içinde saklanır."),
    ("providers.signed_in", "Signed in", "Giriş yapıldı"),
    ("providers.connected_key", "API key saved", "API anahtarı kayıtlı"),
    ("providers.connected_key_preview", "API key {0}", "API anahtarı {0}"),
    ("providers.from_env", "From environment {0}", "Ortam değişkeninden {0}"),
    ("providers.remove", "Remove", "Kaldır"),
    ("providers.add_key", "Add API key", "API anahtarı ekle"),
    ("providers.key_placeholder", "Paste the API key", "API anahtarını yapıştır"),
    ("providers.key_label", "API key for {0}", "{0} için API anahtarı"),
    ("providers.key_rejected", "The key was saved but the test failed: {0}", "Anahtar kaydedildi ama test başarısız: {0}"),
    ("providers.key_ok", "Connected · {0} models", "Bağlandı · {0} model"),
    ("providers.key_hint", "The key is tested right after saving.", "Anahtar kaydedildikten hemen sonra test edilir."),
    ("providers.models_title", "Models", "Modeller"),
    // --- settings: permissions ---
    ("perms.everywhere", "Everywhere", "Her yerde"),
    ("perms.allow_command", "Always allow the command {0}", "{0} komutuna hep izin ver"),
    ("perms.deny_command", "Always deny the command {0}", "{0} komutunu hep reddet"),
    ("perms.allow_network", "Always allow network access to {0}", "{0} ağ erişimine hep izin ver"),
    ("perms.deny_network", "Always deny network access to {0}", "{0} ağ erişimini hep reddet"),
    ("perms.allow_tool", "Always allow the tool {0}", "{0} aracına hep izin ver"),
    ("perms.deny_tool", "Always deny the tool {0}", "{0} aracını hep reddet"),
    ("perms.revoke", "Revoke", "Kaldır"),
    ("perms.revoked", "Revoked", "Kaldırıldı"),
    ("perms.undo", "Undo", "Geri al"),
    // --- shared slash commands ---
    ("slash.model", "show or switch models", "modelleri göster ya da değiştir"),
    ("slash.thinking", "reasoning effort (off..max)", "düşünme düzeyi (off..max)"),
    ("slash.diff", "show the working-tree diff", "çalışma ağacı farkını göster"),
    ("slash.mcp", "list MCP servers and status", "MCP sunucularını ve durumlarını listele"),
    ("slash.skills", "list discovered skills", "bulunan becerileri listele"),
    ("slash.resume", "pick a session of this folder to continue", "bu klasörün bir oturumunu seçip sürdür"),
    ("slash.usage", "usage and cost of this session", "bu oturumun kullanımı ve maliyeti"),
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

    /// The desktop shows the shared command descriptions: every command the
    /// desktop offers has a `slash.<name>` entry whose English text is the
    /// core table's description.
    #[test]
    fn desktop_commands_use_the_shared_descriptions() {
        use locagens_core::commands::{commands_for, Frontend};
        for spec in commands_for(Frontend::Desktop) {
            let key = format!("slash.{}", spec.name);
            let entry = TABLE.iter().find(|(k, _, _)| *k == key).unwrap_or_else(|| panic!("{key} missing"));
            assert_eq!(entry.1, spec.description, "{key}: English must match locagens_core::commands");
            assert_eq!(command_description(Lang::Tr, spec), entry.2);
        }
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
            // e2e_tests.rs is test-only and names config files (`auth.json`).
            if file.file_name().is_some_and(|name| name == "i18n.rs" || name == "prefs.rs" || name == "e2e_tests.rs") {
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

    /// User-visible text must go through the table. Flags string literals
    /// that are rsx text nodes (`span { class: "x", "Text" }` or a bare
    /// `"Text"` line) or `title` / `placeholder` / `aria-label` / `alt`
    /// attributes. Interpolated strings need their own words to be flagged
    /// (`"{x} tok"`), pure data (`"{path}"`) is fine.
    fn untranslated_literals(source: &str) -> Vec<String> {
        const ALLOWED: &[&str] = &["Locagens"];
        const ATTRS: &[&str] = &["title", "placeholder", "aria_label", "alt", "\"aria-label\""];
        let mut found = Vec::new();
        for line in source.lines() {
            let line = line.trim();
            if line.starts_with("//") || line.starts_with("///") {
                continue;
            }
            let mut literals: Vec<(usize, String)> = Vec::new();
            let mut rest = line;
            let mut offset = 0;
            while let Some(start) = rest.find('"') {
                let after = &rest[start + 1..];
                let Some(len) = after.find('"') else { break };
                literals.push((offset + start, after[..len].to_string()));
                offset += start + len + 2;
                rest = &line[offset..];
            }
            for (pos, literal) in literals {
                let words = strip_braces(&literal);
                let has_words = words.chars().any(char::is_alphabetic);
                let before = line[..pos].trim_end();
                let attr_value = before.ends_with(':');
                let flagged = if attr_value {
                    let name = before.trim_end_matches(':').rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == '"' || c == '-')).next().unwrap_or("");
                    ATTRS.contains(&name) && has_words
                } else {
                    let text_node = before.is_empty() || before.ends_with('{') || before.ends_with(',') || before.ends_with('}');
                    let after_literal = line[pos + literal.len() + 2..].trim_start();
                    let ends_node = after_literal.is_empty() || after_literal.starts_with('}');
                    let looks_like_text = literal.trim().chars().next().is_some_and(char::is_uppercase)
                        || (literal.contains('{') && has_words);
                    text_node && ends_node && looks_like_text
                };
                if flagged && !ALLOWED.contains(&literal.as_str()) {
                    found.push(line.to_string());
                }
            }
        }
        found
    }

    fn strip_braces(text: &str) -> String {
        let mut out = String::new();
        let mut depth = 0;
        for c in text.chars() {
            match c {
                '{' => depth += 1,
                '}' if depth > 0 => depth -= 1,
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn scanner_flags_literal_text_nodes_and_attributes() {
        assert_eq!(untranslated_literals("span { class: \"x\", \"Code review\" }").len(), 1);
        assert_eq!(untranslated_literals("\"No findings.\"").len(), 1);
        assert_eq!(untranslated_literals("title: \"Copy\",").len(), 1);
        assert_eq!(untranslated_literals("placeholder: \"Search\",").len(), 1);
        assert_eq!(untranslated_literals("\"{x} tok\"").len(), 1);
        assert!(untranslated_literals("span { class: \"badge ok\", \"{store.t(\\\"a.b\\\")}\" }").is_empty());
        assert!(untranslated_literals("title: \"{path}\",").is_empty());
        assert!(untranslated_literals("title: store.t(\"a.b\"),").is_empty());
        assert!(untranslated_literals("div { class: \"Foo bar\" }").is_empty());
        assert!(untranslated_literals("span { class: \"brand-title\", \"Locagens\" }").is_empty());
    }

    #[test]
    fn no_user_visible_literals_bypass_the_table() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs(&src, &mut files);
        let mut offenders = Vec::new();
        for file in files {
            if file.file_name().is_some_and(|name| name == "i18n.rs" || name == "prefs.rs" || name == "theme.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            let production = text.split("#[cfg(test)]").next().unwrap_or("");
            for hit in untranslated_literals(production) {
                offenders.push(format!("{}: {hit}", file.file_name().unwrap().to_string_lossy()));
            }
        }
        assert!(offenders.is_empty(), "user-visible strings must use i18n.rs: {offenders:#?}");
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
