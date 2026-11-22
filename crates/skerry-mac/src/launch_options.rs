//! The Launch Dialog's choices, made as SealSkin's web launcher makes them
//! (`client/src/ui/home.js`, `lib/gpus.js`, `lib/homes.js`, `lib/languages.js`; ARCH-016):
//! which fields show, what each offers, what is pre-selected, what a launch sends, and what
//! Copy link writes. Everything here is pure so it can be tested without a server or a window.

use crate::sealskin::{GpuInfo, LaunchParams, RemoteApp, RemoteSession, ServerStatus};
use serde::{Deserialize, Serialize};
use skerry_ui::launch_dialog::LaunchOption;

/// The locale the server starts sessions in when none is asked for.
pub const DEFAULT_LANGUAGE: &str = "en_US.UTF-8";

/// The session locales SealSkin's launcher offers (`lib/languages.js`), after the default.
const LOCALES: &[&str] = &[
    "aa_DJ.UTF-8",
    "aa_ER.UTF-8",
    "aa_ET.UTF-8",
    "af_ZA.UTF-8",
    "am_ET.UTF-8",
    "an_ES.UTF-8",
    "ar_AE.UTF-8",
    "ar_BH.UTF-8",
    "ar_DZ.UTF-8",
    "ar_EG.UTF-8",
    "ar_IN.UTF-8",
    "ar_IQ.UTF-8",
    "ar_JO.UTF-8",
    "ar_KW.UTF-8",
    "ar_LB.UTF-8",
    "ar_LY.UTF-8",
    "ar_MA.UTF-8",
    "ar_OM.UTF-8",
    "ar_QA.UTF-8",
    "ar_SA.UTF-8",
    "ar_SD.UTF-8",
    "ar_SY.UTF-8",
    "ar_TN.UTF-8",
    "ar_YE.UTF-8",
    "as_IN.UTF-8",
    "ast_ES.UTF-8",
    "ayc_PE.UTF-8",
    "az_AZ.UTF-8",
    "be_BY.UTF-8",
    "bem_ZM.UTF-8",
    "ber_DZ.UTF-8",
    "ber_MA.UTF-8",
    "bg_BG.UTF-8",
    "bho_IN.UTF-8",
    "bn_BD.UTF-8",
    "bn_IN.UTF-8",
    "bo_CN.UTF-8",
    "bo_IN.UTF-8",
    "br_FR.UTF-8",
    "brx_IN.UTF-8",
    "bs_BA.UTF-8",
    "byn_ER.UTF-8",
    "ca_AD.UTF-8",
    "ca_ES.UTF-8",
    "ca_FR.UTF-8",
    "ca_IT.UTF-8",
    "crh_UA.UTF-8",
    "csb_PL.UTF-8",
    "cs_CZ.UTF-8",
    "cv_RU.UTF-8",
    "cy_GB.UTF-8",
    "da_DK.UTF-8",
    "de_AT.UTF-8",
    "de_BE.UTF-8",
    "de_CH.UTF-8",
    "de_DE.UTF-8",
    "de_LU.UTF-8",
    "doi_IN.UTF-8",
    "dv_MV.UTF-8",
    "dz_BT.UTF-8",
    "el_CY.UTF-8",
    "el_GR.UTF-8",
    "en_AG.UTF-8",
    "en_AU.UTF-8",
    "en_BW.UTF-8",
    "en_CA.UTF-8",
    "en_DK.UTF-8",
    "en_GB.UTF-8",
    "en_HK.UTF-8",
    "en_IE.UTF-8",
    "en_IN.UTF-8",
    "en_NG.UTF-8",
    "en_NZ.UTF-8",
    "en_PH.UTF-8",
    "en_SG.UTF-8",
    "en_ZA.UTF-8",
    "en_ZM.UTF-8",
    "en_ZW.UTF-8",
    "es_AR.UTF-8",
    "es_BO.UTF-8",
    "es_CL.UTF-8",
    "es_CO.UTF-8",
    "es_CR.UTF-8",
    "es_CU.UTF-8",
    "es_DO.UTF-8",
    "es_EC.UTF-8",
    "es_ES.UTF-8",
    "es_GT.UTF-8",
    "es_HN.UTF-8",
    "es_MX.UTF-8",
    "es_NI.UTF-8",
    "es_PA.UTF-8",
    "es_PE.UTF-8",
    "es_PR.UTF-8",
    "es_PY.UTF-8",
    "es_SV.UTF-8",
    "es_US.UTF-8",
    "es_UY.UTF-8",
    "es_VE.UTF-8",
    "et_EE.UTF-8",
    "eu_ES.UTF-8",
    "fa_IR.UTF-8",
    "ff_SN.UTF-8",
    "fi_FI.UTF-8",
    "fil_PH.UTF-8",
    "fo_FO.UTF-8",
    "fr_BE.UTF-8",
    "fr_CA.UTF-8",
    "fr_CH.UTF-8",
    "fr_FR.UTF-8",
    "fr_LU.UTF-8",
    "fur_IT.UTF-8",
    "fy_DE.UTF-8",
    "fy_NL.UTF-8",
    "ga_IE.UTF-8",
    "gd_GB.UTF-8",
    "gez_ER.UTF-8",
    "gez_ET.UTF-8",
    "gl_ES.UTF-8",
    "gu_IN.UTF-8",
    "gv_GB.UTF-8",
    "ha_NG.UTF-8",
    "he_IL.UTF-8",
    "hi_IN.UTF-8",
    "hne_IN.UTF-8",
    "hr_HR.UTF-8",
    "hsb_DE.UTF-8",
    "ht_HT.UTF-8",
    "hu_HU.UTF-8",
    "hy_AM.UTF-8",
    "ia_FR.UTF-8",
    "id_ID.UTF-8",
    "ig_NG.UTF-8",
    "ik_CA.UTF-8",
    "is_IS.UTF-8",
    "it_CH.UTF-8",
    "it_IT.UTF-8",
    "iu_CA.UTF-8",
    "ja_JP.UTF-8",
    "ka_GE.UTF-8",
    "kk_KZ.UTF-8",
    "kl_GL.UTF-8",
    "km_KH.UTF-8",
    "kn_IN.UTF-8",
    "kok_IN.UTF-8",
    "ko_KR.UTF-8",
    "ks_IN.UTF-8",
    "ku_TR.UTF-8",
    "kw_GB.UTF-8",
    "ky_KG.UTF-8",
    "lb_LU.UTF-8",
    "lg_UG.UTF-8",
    "li_BE.UTF-8",
    "lij_IT.UTF-8",
    "li_NL.UTF-8",
    "lo_LA.UTF-8",
    "lt_LT.UTF-8",
    "lv_LV.UTF-8",
    "mag_IN.UTF-8",
    "mai_IN.UTF-8",
    "mg_MG.UTF-8",
    "mhr_RU.UTF-8",
    "mi_NZ.UTF-8",
    "mk_MK.UTF-8",
    "ml_IN.UTF-8",
    "mni_IN.UTF-8",
    "mn_MN.UTF-8",
    "mr_IN.UTF-8",
    "ms_MY.UTF-8",
    "mt_MT.UTF-8",
    "my_MM.UTF-8",
    "nb_NO.UTF-8",
    "nds_DE.UTF-8",
    "nds_NL.UTF-8",
    "ne_NP.UTF-8",
    "nhn_MX.UTF-8",
    "niu_NU.UTF-8",
    "niu_NZ.UTF-8",
    "nl_AW.UTF-8",
    "nl_BE.UTF-8",
    "nl_NL.UTF-8",
    "nn_NO.UTF-8",
    "nr_ZA.UTF-8",
    "nso_ZA.UTF-8",
    "oc_FR.UTF-8",
    "om_ET.UTF-8",
    "om_KE.UTF-8",
    "or_IN.UTF-8",
    "os_RU.UTF-8",
    "pa_IN.UTF-8",
    "pa_PK.UTF-8",
    "pl_PL.UTF-8",
    "ps_AF.UTF-8",
    "pt_BR.UTF-8",
    "pt_PT.UTF-8",
    "ro_RO.UTF-8",
    "ru_RU.UTF-8",
    "ru_UA.UTF-8",
    "rw_RW.UTF-8",
    "sa_IN.UTF-8",
    "sat_IN.UTF-8",
    "sc_IT.UTF-8",
    "sd_IN.UTF-8",
    "se_NO.UTF-8",
    "shs_CA.UTF-8",
    "sid_ET.UTF-8",
    "si_LK.UTF-8",
    "sk_SK.UTF-8",
    "sl_SI.UTF-8",
    "so_DJ.UTF-8",
    "so_ET.UTF-8",
    "so_KE.UTF-8",
    "so_SO.UTF-8",
    "sq_AL.UTF-8",
    "sq_MK.UTF-8",
    "sr_ME.UTF-8",
    "sr_RS.UTF-8",
    "ss_ZA.UTF-8",
    "st_ZA.UTF-8",
    "sv_FI.UTF-8",
    "sv_SE.UTF-8",
    "sw_KE.UTF-8",
    "sw_TZ.UTF-8",
    "szl_PL.UTF-8",
    "ta_IN.UTF-8",
    "ta_LK.UTF-8",
    "te_IN.UTF-8",
    "tg_TJ.UTF-8",
    "th_TH.UTF-8",
    "ti_ER.UTF-8",
    "ti_ET.UTF-8",
    "tig_ER.UTF-8",
    "tk_TM.UTF-8",
    "tl_PH.UTF-8",
    "tn_ZA.UTF-8",
    "tr_CY.UTF-8",
    "tr_TR.UTF-8",
    "ts_ZA.UTF-8",
    "tt_RU.UTF-8",
    "ug_CN.UTF-8",
    "uk_UA.UTF-8",
    "unm_US.UTF-8",
    "ur_IN.UTF-8",
    "ur_PK.UTF-8",
    "uz_UZ.UTF-8",
    "ve_ZA.UTF-8",
    "vi_VN.UTF-8",
    "wa_BE.UTF-8",
    "wae_CH.UTF-8",
    "wal_ET.UTF-8",
    "wo_SN.UTF-8",
    "xh_ZA.UTF-8",
    "yi_US.UTF-8",
    "yo_NG.UTF-8",
    "yue_HK.UTF-8",
    "zh_CN.UTF-8",
    "zh_HK.UTF-8",
    "zh_SG.UTF-8",
    "zh_TW.UTF-8",
    "zu_ZA.UTF-8",
];

/// Storage ids besides named homes.
const AUTO: &str = "auto";
const CLEANROOM: &str = "cleanroom";
/// A home every user has, which holds shared files and is never offered.
use crate::sealskin::SHARED_FILES_HOME;

/// What the server lets this user launch with, read once per Library load.
#[derive(Clone, Debug, Default)]
pub struct LaunchContext {
    pub status: ServerStatus,
    pub homes: Vec<String>,
}

/// The options last launched with for one app; every field is optional so older or partial
/// records load, and a value that no longer holds falls back to the default.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct LaunchMemory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vpn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vpn_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wayland_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<bool>,
}

/// The dialog's current choice in each field; `""` is "None" for GPU and "No VPN" for VPN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchSelection {
    pub home_dir: String,
    pub gpu: String,
    pub vpn: String,
    pub vpn_endpoint: String,
    pub language: String,
    pub wayland: bool,
    pub room: bool,
}

fn option(id: impl Into<String>, label: impl Into<String>) -> LaunchOption {
    LaunchOption {
        id: id.into(),
        label: label.into(),
    }
}

impl LaunchContext {
    /// Storage shows only for a user with persistent storage and an app that keeps homes.
    fn has_storage(&self, app: &RemoteApp) -> bool {
        self.status.settings.persistent_storage && app.home_directories
    }

    /// The user's own named homes: not the shared-files home, not an app's Auto home.
    fn named_homes(&self, app: &RemoteApp) -> Vec<&str> {
        if app.is_meta_app {
            return Vec::new();
        }
        self.homes
            .iter()
            .map(String::as_str)
            .filter(|home| *home != SHARED_FILES_HOME && !home.starts_with("auto-"))
            .collect()
    }

    /// Auto, Cleanroom, then named homes; empty (the field hides) without storage.
    pub fn storage_options(&self, app: &RemoteApp) -> Vec<LaunchOption> {
        if !self.has_storage(app) {
            return Vec::new();
        }
        let mut options = vec![
            option(AUTO, "Auto (per-app persistence)"),
            option(CLEANROOM, "Cleanroom (temporary)"),
        ];
        options.extend(
            self.named_homes(app)
                .into_iter()
                .map(|home| option(home, home)),
        );
        options
    }

    /// "None", then the GPUs whose kind the app runs on; empty (the field hides) when the
    /// user may not use a GPU or none fits.
    pub fn gpu_options(&self, app: &RemoteApp) -> Vec<LaunchOption> {
        if !self.status.settings.gpu {
            return Vec::new();
        }
        let fitting: Vec<LaunchOption> = self
            .status
            .gpus
            .iter()
            .filter(|gpu| supports(app, gpu))
            .map(|gpu| option(gpu.device.clone(), gpu_label(gpu)))
            .collect();
        if fitting.is_empty() {
            return fitting;
        }
        let mut options = vec![option("", "None")];
        options.extend(fitting);
        options
    }

    /// "No VPN", then the profiles open to the user; empty (the field hides) when there are none.
    pub fn vpn_options(&self) -> Vec<LaunchOption> {
        if self.status.vpns.is_empty() {
            return Vec::new();
        }
        let mut options = vec![option("", "No VPN")];
        options.extend(
            self.status
                .vpns
                .iter()
                .map(|vpn| option(vpn.id.clone(), vpn.name.clone())),
        );
        options
    }

    /// The endpoints of VPN profile `vpn`; the dialog shows them when there are two or more.
    pub fn vpn_endpoints(&self, vpn: &str) -> Vec<LaunchOption> {
        self.status
            .vpns
            .iter()
            .find(|choice| choice.id == vpn)
            .map(|choice| {
                choice
                    .endpoints
                    .iter()
                    .map(|endpoint| option(endpoint.id.clone(), endpoint.label.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The options a dialog opens with: those last used where they still hold, else the
    /// defaults (Auto, no GPU, no VPN, the locale nearest the Mac's, Wayland on).
    pub fn selection(
        &self,
        app: &RemoteApp,
        memory: &LaunchMemory,
        default_language: &str,
    ) -> LaunchSelection {
        let offered = |options: &[LaunchOption], wanted: &Option<String>| {
            wanted
                .as_ref()
                .filter(|wanted| options.iter().any(|option| &option.id == *wanted))
                .cloned()
        };
        let home_dir = if self.has_storage(app) {
            offered(&self.storage_options(app), &memory.home_dir).unwrap_or_else(|| AUTO.into())
        } else {
            CLEANROOM.into()
        };
        let gpu = offered(&self.gpu_options(app), &memory.gpu).unwrap_or_default();
        let vpn = offered(&self.vpn_options(), &memory.vpn).unwrap_or_default();
        let endpoints = self.vpn_endpoints(&vpn);
        let vpn_endpoint = offered(&endpoints, &memory.vpn_endpoint)
            .or_else(|| endpoints.first().map(|endpoint| endpoint.id.clone()))
            .unwrap_or_default();
        let language = memory
            .language
            .clone()
            .filter(|language| is_language(language))
            .unwrap_or_else(|| default_language.into());
        LaunchSelection {
            home_dir,
            gpu,
            vpn,
            vpn_endpoint,
            language,
            wayland: memory.wayland_mode != Some(false),
            room: memory.room == Some(true),
        }
    }

    /// What a launch with `selection` sends. Auto names the app's own home, `auto-<app>`,
    /// created first when the user doesn't have it yet (a meta app's is made by the server).
    pub fn params(
        &self,
        app: &RemoteApp,
        selection: &LaunchSelection,
        timezone: Option<String>,
    ) -> LaunchParams {
        let home_name = self.resolved_home(app, selection);
        let create_home = self.has_storage(app)
            && selection.home_dir == AUTO
            && !app.is_meta_app
            && !self.homes.contains(&home_name);
        let chosen = |value: &str| (!value.is_empty()).then(|| value.to_string());
        let vpn = chosen(&selection.vpn);
        LaunchParams {
            launch_id: None,
            home_name: Some(home_name),
            create_home,
            language: Some(selection.language.clone()),
            timezone,
            selected_gpu: chosen(&selection.gpu),
            vpn_endpoint: vpn.as_ref().and_then(|_| chosen(&selection.vpn_endpoint)),
            vpn,
            wayland_mode: selection.wayland,
            launch_in_room_mode: selection.room,
        }
    }

    /// The storage a launch with `selection` uses: the app's Auto home (`auto-<app>`),
    /// `cleanroom`, or a named home.
    pub fn resolved_home(&self, app: &RemoteApp, selection: &LaunchSelection) -> String {
        if !self.has_storage(app) || selection.home_dir.is_empty() {
            CLEANROOM.to_string()
        } else if selection.home_dir == AUTO {
            auto_home_name(&app.name)
        } else {
            selection.home_dir.clone()
        }
    }

    /// The running session a launch with `selection` would duplicate, as SealSkin's web app
    /// address page decides (`open.js`): the same app, the same storage, and the same room
    /// flag. GPU, VPN, language, and Wayland only describe how a new session starts. The
    /// oldest match wins.
    pub fn matching_session<'a>(
        &self,
        app: &RemoteApp,
        selection: &LaunchSelection,
        sessions: &'a [RemoteSession],
    ) -> Option<&'a RemoteSession> {
        let home = self.resolved_home(app, selection);
        let same_storage = |session_home: &str| {
            if home == CLEANROOM {
                session_home.is_empty() || session_home == CLEANROOM
            } else {
                session_home == home
            }
        };
        sessions
            .iter()
            .filter(|session| {
                session.app_id == app.id
                    && same_storage(&session.home)
                    && session.is_collaboration == selection.room
            })
            .min_by(|a, b| {
                a.created_at
                    .total_cmp(&b.created_at)
                    .then_with(|| a.session_id.cmp(&b.session_id))
            })
    }

    /// The query Copy link adds to `/app/<id>/`, as the web launcher writes it: only what
    /// differs from a default launch.
    pub fn link_query(
        &self,
        app: &RemoteApp,
        selection: &LaunchSelection,
        default_language: &str,
    ) -> Vec<(&'static str, String)> {
        let mut query = Vec::new();
        if self.has_storage(app) && !selection.home_dir.is_empty() && selection.home_dir != AUTO {
            query.push(("home", selection.home_dir.clone()));
        }
        if !selection.gpu.is_empty() {
            query.push(("gpu", selection.gpu.clone()));
        }
        if !selection.vpn.is_empty() {
            query.push(("vpn", selection.vpn.clone()));
            if !selection.vpn_endpoint.is_empty() {
                query.push(("vpn_endpoint", selection.vpn_endpoint.clone()));
            }
        }
        if selection.room {
            query.push(("room", "1".into()));
        }
        if !selection.language.is_empty() && selection.language != default_language {
            query.push(("lang", selection.language.clone()));
        }
        if !selection.wayland {
            query.push(("wayland", "0".into()));
        }
        query
    }
}

impl LaunchSelection {
    /// What is remembered for the app after it launches with this selection.
    pub fn memory(&self) -> LaunchMemory {
        let chosen = |value: &str| (!value.is_empty()).then(|| value.to_string());
        LaunchMemory {
            home_dir: chosen(&self.home_dir),
            gpu: chosen(&self.gpu),
            vpn: chosen(&self.vpn),
            vpn_endpoint: chosen(&self.vpn).and_then(|_| chosen(&self.vpn_endpoint)),
            language: chosen(&self.language),
            wayland_mode: Some(self.wayland),
            room: Some(self.room),
        }
    }
}

/// An NVIDIA GPU runs NVIDIA apps; every other driver (i915, xe, amdgpu, …) serves DRI3.
fn supports(app: &RemoteApp, gpu: &GpuInfo) -> bool {
    if gpu.driver == "nvidia" {
        app.nvidia_support
    } else {
        app.dri3_support
    }
}

/// "model (node)" when the model is known, else "node (driver)", where node is the device's
/// last path part: `intel-gpu (i915)`, `NVIDIA L40S (renderD128)`.
pub fn gpu_label(gpu: &GpuInfo) -> String {
    let node = gpu.device.rsplit('/').next().unwrap_or(&gpu.device);
    match gpu.model.as_deref().filter(|model| !model.is_empty()) {
        Some(model) => format!("{model} ({node})"),
        None if gpu.driver.is_empty() => node.to_string(),
        None => format!("{node} ({})", gpu.driver),
    }
}

/// The app's Auto home: `auto-` and its name in lower case, spaces and underscores as
/// hyphens, anything else outside `a-z0-9-` dropped (`VS Code` → `auto-vs-code`).
pub fn auto_home_name(app_name: &str) -> String {
    let mut name = String::from("auto-");
    let mut in_gap = false;
    for c in app_name.to_lowercase().chars() {
        if c.is_whitespace() || c == '_' {
            if !in_gap {
                name.push('-');
            }
            in_gap = true;
            continue;
        }
        in_gap = false;
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            name.push(c);
        }
    }
    name
}

fn is_language(id: &str) -> bool {
    id == DEFAULT_LANGUAGE || LOCALES.contains(&id)
}

/// Every session locale: the default first, then the rest, labelled by `name` (the Mac's name
/// for a locale such as `de_DE`, if it has one): "German (Germany) · de_DE".
pub fn language_options(name: impl Fn(&str) -> Option<String>) -> Vec<LaunchOption> {
    let mut options = vec![option(DEFAULT_LANGUAGE, "Default (en_US)")];
    options.extend(LOCALES.iter().map(|id| {
        let locale = id.trim_end_matches(".UTF-8");
        let label = match name(locale) {
            Some(name) if !name.is_empty() && name != locale => format!("{name} · {locale}"),
            _ => locale.to_string(),
        };
        option(*id, label)
    }));
    options
}

/// The session locale nearest `locale` (`pt-BR`, `en_US`), as the web launcher picks it:
/// the exact match, else the language's main country, else any country of the language,
/// else the default.
pub fn default_language(locale: &str) -> String {
    let locale = if locale.is_empty() { "en" } else { locale };
    let mut parts = locale.splitn(2, ['_', '-']);
    let lang = parts.next().unwrap_or("en").to_lowercase();
    let region = parts
        .next()
        .and_then(|rest| rest.split(['_', '-']).next())
        .unwrap_or("")
        .to_uppercase();
    let exact = format!("{lang}_{region}.UTF-8");
    if is_language(&exact) {
        return exact;
    }
    let primary = match lang.as_str() {
        "es" => Some("ES"),
        "fr" => Some("FR"),
        "pt" => Some("BR"),
        "de" => Some("DE"),
        "it" => Some("IT"),
        "ru" => Some("RU"),
        "ja" => Some("JP"),
        "ko" => Some("KR"),
        "th" => Some("TH"),
        "zh" if region == "TW" || region == "HK" => Some(region.as_str()),
        "zh" => Some("CN"),
        _ => None,
    };
    if let Some(primary) = primary {
        let main = format!("{lang}_{primary}.UTF-8");
        if is_language(&main) {
            return main;
        }
    }
    let prefix = format!("{lang}_");
    std::iter::once(DEFAULT_LANGUAGE)
        .chain(LOCALES.iter().copied())
        .find(|id| id.starts_with(&prefix))
        .unwrap_or(DEFAULT_LANGUAGE)
        .to_string()
}

/// The Mac's language and region as `lang-REGION` (`de-DE`), the form `default_language` reads.
pub fn mac_locale() -> String {
    use objc2_foundation::NSLocale;
    let locale = NSLocale::currentLocale();
    let language = locale.languageCode().to_string();
    match locale.regionCode() {
        Some(region) => format!("{language}-{region}"),
        None => language,
    }
}

/// The Mac's time zone (`Europe/Berlin`), which sessions start in.
pub fn mac_timezone() -> Option<String> {
    let name = objc2_foundation::NSTimeZone::localTimeZone()
        .name()
        .to_string();
    (!name.is_empty()).then_some(name)
}

/// The Mac's name for a locale such as `de_DE`, in the Mac's own language.
pub fn mac_locale_name(locale: &str) -> Option<String> {
    use objc2_foundation::{NSLocale, NSString};
    let name = NSLocale::currentLocale()
        .localizedStringForLocaleIdentifier(&NSString::from_str(locale))
        .to_string();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sealskin::{UserSettings, VpnChoice, VpnEndpoint};

    fn app(name: &str, nvidia: bool, dri3: bool) -> RemoteApp {
        serde_json::from_value(serde_json::json!({
            "id": name.to_lowercase(),
            "name": name,
            "home_directories": true,
            "nvidia_support": nvidia,
            "dri3_support": dri3,
        }))
        .unwrap()
    }

    fn gpu(device: &str, driver: &str, model: Option<&str>) -> GpuInfo {
        GpuInfo {
            device: device.into(),
            driver: driver.into(),
            model: model.map(Into::into),
        }
    }

    /// A Kubernetes server like the user's: storage, GPU, and VPN rights, an Intel GPU
    /// template, an NVIDIA GPU, named homes, and a VPN with two endpoints.
    fn context() -> LaunchContext {
        LaunchContext {
            status: ServerStatus {
                settings: UserSettings {
                    persistent_storage: true,
                    gpu: true,
                    vpn: true,
                    ..UserSettings::default()
                },
                gpus: vec![
                    gpu("intel-gpu", "i915", None),
                    gpu("/dev/dri/renderD129", "nvidia", Some("NVIDIA L40S")),
                ],
                vpns: vec![VpnChoice {
                    id: "mullvad".into(),
                    name: "Mullvad".into(),
                    endpoints: vec![
                        VpnEndpoint {
                            id: "de".into(),
                            label: "de-fra".into(),
                        },
                        VpnEndpoint {
                            id: "nl".into(),
                            label: "nl-ams".into(),
                        },
                    ],
                }],
                ..ServerStatus::default()
            },
            homes: vec!["work".into(), "auto-zen".into(), SHARED_FILES_HOME.into()],
        }
    }

    fn ids(options: &[LaunchOption]) -> Vec<&str> {
        options.iter().map(|option| option.id.as_str()).collect()
    }

    #[test]
    fn fields_offer_what_the_web_launcher_offers() {
        let ctx = context();
        let zen = app("Zen", false, true);
        assert_eq!(
            ids(&ctx.storage_options(&zen)),
            ["auto", "cleanroom", "work"]
        );
        // Only the DRI3 GPU fits a DRI3-only app; the label is "id (driver)" without a model.
        let gpus = ctx.gpu_options(&zen);
        assert_eq!(ids(&gpus), ["", "intel-gpu"]);
        assert_eq!(gpus[1].label, "intel-gpu (i915)");
        let both = app("Blender", true, true);
        assert_eq!(ctx.gpu_options(&both)[2].label, "NVIDIA L40S (renderD129)");
        assert_eq!(ids(&ctx.vpn_options()), ["", "mullvad"]);
        assert_eq!(ids(&ctx.vpn_endpoints("mullvad")), ["de", "nl"]);
        assert!(ctx.vpn_endpoints("").is_empty());
    }

    #[test]
    fn fields_hide_without_rights_or_fitting_gpus() {
        let mut ctx = context();
        assert!(ctx.gpu_options(&app("Office", false, false)).is_empty());
        ctx.status.settings = UserSettings::default();
        ctx.status.vpns.clear();
        let zen = app("Zen", false, true);
        assert!(ctx.storage_options(&zen).is_empty());
        assert!(ctx.gpu_options(&zen).is_empty());
        assert!(ctx.vpn_options().is_empty());
        // Without storage the launch is a cleanroom one.
        let selection = ctx.selection(&zen, &LaunchMemory::default(), DEFAULT_LANGUAGE);
        assert_eq!(selection.home_dir, "cleanroom");
        let params = ctx.params(&zen, &selection, None);
        assert_eq!(params.home_name.as_deref(), Some("cleanroom"));
        assert!(!params.create_home);
    }

    #[test]
    fn defaults_then_remembered_options_where_they_still_hold() {
        let ctx = context();
        let zen = app("Zen", false, true);
        let fresh = ctx.selection(&zen, &LaunchMemory::default(), "de_DE.UTF-8");
        assert_eq!(
            fresh,
            LaunchSelection {
                home_dir: "auto".into(),
                gpu: String::new(),
                vpn: String::new(),
                vpn_endpoint: String::new(),
                language: "de_DE.UTF-8".into(),
                wayland: true,
                room: false,
            }
        );
        let remembered = LaunchSelection {
            home_dir: "work".into(),
            gpu: "intel-gpu".into(),
            vpn: "mullvad".into(),
            vpn_endpoint: "nl".into(),
            language: "ja_JP.UTF-8".into(),
            wayland: false,
            room: true,
        };
        assert_eq!(
            ctx.selection(&zen, &remembered.memory(), "de_DE.UTF-8"),
            remembered
        );
        // Gone homes, GPUs, VPNs, and endpoints fall back to the defaults.
        let stale = LaunchMemory {
            home_dir: Some("deleted".into()),
            gpu: Some("/dev/dri/renderD200".into()),
            vpn: Some("mullvad".into()),
            vpn_endpoint: Some("se".into()),
            language: Some("xx_XX.UTF-8".into()),
            ..LaunchMemory::default()
        };
        let selection = ctx.selection(&zen, &stale, "de_DE.UTF-8");
        assert_eq!(selection.home_dir, "auto");
        assert_eq!(selection.gpu, "");
        assert_eq!(selection.vpn_endpoint, "de");
        assert_eq!(selection.language, "de_DE.UTF-8");
    }

    fn session(id: &str, app: &str, home: &str, room: bool, created_at: f64) -> RemoteSession {
        serde_json::from_value(serde_json::json!({
            "session_id": id,
            "app_id": app,
            "app_name": app,
            "session_url": format!("/{id}/"),
            "home": home,
            "is_collaboration": room,
            "created_at": created_at,
        }))
        .unwrap()
    }

    fn select(home: &str, room: bool) -> LaunchSelection {
        LaunchSelection {
            home_dir: home.into(),
            gpu: String::new(),
            vpn: String::new(),
            vpn_endpoint: String::new(),
            language: DEFAULT_LANGUAGE.into(),
            wayland: true,
            room,
        }
    }

    #[test]
    fn resolved_home_is_auto_name_cleanroom_or_named_home() {
        let ctx = context();
        let code = app("VS Code", false, true);
        assert_eq!(
            ctx.resolved_home(&code, &select("auto", false)),
            "auto-vs-code"
        );
        assert_eq!(
            ctx.resolved_home(&code, &select("cleanroom", false)),
            "cleanroom"
        );
        assert_eq!(ctx.resolved_home(&code, &select("work", false)), "work");
    }

    #[test]
    fn resolved_home_without_storage_is_cleanroom() {
        let mut ctx = context();
        let zen = app("Zen", false, true);
        assert_eq!(ctx.resolved_home(&zen, &select("", false)), "cleanroom");
        ctx.status.settings.persistent_storage = false;
        assert_eq!(ctx.resolved_home(&zen, &select("auto", false)), "cleanroom");
    }

    #[test]
    fn matching_session_requires_same_app_storage_and_room() {
        let ctx = context();
        let zen = app("Zen", false, true);
        let sessions = [
            session("s1", "zen", "work", false, 10.0),
            session("s2", "zen", "auto-zen", false, 20.0),
        ];
        let found = ctx.matching_session(&zen, &select("auto", false), &sessions);
        assert_eq!(found.map(|s| s.session_id.as_str()), Some("s2"));
        let found = ctx.matching_session(&zen, &select("work", false), &sessions);
        assert_eq!(found.map(|s| s.session_id.as_str()), Some("s1"));
        assert!(ctx
            .matching_session(&zen, &select("cleanroom", false), &sessions)
            .is_none());
    }

    #[test]
    fn matching_session_skips_other_apps_and_room_mismatch() {
        let ctx = context();
        let zen = app("Zen", false, true);
        let sessions = [
            session("s1", "firefox", "auto-zen", false, 10.0),
            session("s2", "zen", "auto-zen", true, 20.0),
        ];
        assert!(ctx
            .matching_session(&zen, &select("auto", false), &sessions)
            .is_none());
        let found = ctx.matching_session(&zen, &select("auto", true), &sessions);
        assert_eq!(found.map(|s| s.session_id.as_str()), Some("s2"));
    }

    #[test]
    fn matching_session_ignores_gpu_language_and_wayland() {
        let ctx = context();
        let zen = app("Zen", false, true);
        let sessions = [session("s1", "zen", "auto-zen", false, 10.0)];
        let selection = LaunchSelection {
            gpu: "intel-gpu".into(),
            language: "ja_JP.UTF-8".into(),
            wayland: false,
            vpn: "mullvad".into(),
            vpn_endpoint: "nl".into(),
            ..select("auto", false)
        };
        assert!(ctx.matching_session(&zen, &selection, &sessions).is_some());
    }

    #[test]
    fn matching_session_prefers_the_oldest() {
        let ctx = context();
        let zen = app("Zen", false, true);
        let sessions = [
            session("s3", "zen", "auto-zen", false, 30.0),
            session("s2", "zen", "auto-zen", false, 10.0),
            session("s1", "zen", "auto-zen", false, 10.0),
        ];
        let found = ctx.matching_session(&zen, &select("auto", false), &sessions);
        assert_eq!(found.map(|s| s.session_id.as_str()), Some("s1"));
    }

    #[test]
    fn cleanroom_matches_a_session_without_home() {
        let ctx = context();
        let zen = app("Zen", false, true);
        let sessions = [
            session("s1", "zen", "", false, 10.0),
            session("s2", "zen", "cleanroom", false, 5.0),
        ];
        let found = ctx.matching_session(&zen, &select("cleanroom", false), &sessions);
        assert_eq!(found.map(|s| s.session_id.as_str()), Some("s2"));
    }

    #[test]
    fn auto_storage_names_and_creates_the_apps_own_home() {
        let ctx = context();
        let code = app("VS Code", false, true);
        let selection = ctx.selection(&code, &LaunchMemory::default(), DEFAULT_LANGUAGE);
        let params = ctx.params(&code, &selection, Some("Europe/Berlin".into()));
        assert_eq!(params.home_name.as_deref(), Some("auto-vs-code"));
        assert!(params.create_home);
        assert_eq!(params.timezone.as_deref(), Some("Europe/Berlin"));
        // An Auto home that exists is used as it is.
        let zen = app("Zen", false, true);
        let params = ctx.params(&zen, &selection, None);
        assert_eq!(params.home_name.as_deref(), Some("auto-zen"));
        assert!(!params.create_home);
    }

    #[test]
    fn launch_sends_gpu_and_vpn_only_when_chosen() {
        let ctx = context();
        let zen = app("Zen", false, true);
        let mut selection = ctx.selection(&zen, &LaunchMemory::default(), DEFAULT_LANGUAGE);
        let params = ctx.params(&zen, &selection, None);
        assert_eq!(
            (params.selected_gpu, params.vpn, params.vpn_endpoint),
            (None, None, None)
        );
        selection.gpu = "intel-gpu".into();
        selection.vpn = "mullvad".into();
        selection.vpn_endpoint = "nl".into();
        let params = ctx.params(&zen, &selection, None);
        assert_eq!(params.selected_gpu.as_deref(), Some("intel-gpu"));
        assert_eq!(params.vpn.as_deref(), Some("mullvad"));
        assert_eq!(params.vpn_endpoint.as_deref(), Some("nl"));
    }

    #[test]
    fn copy_link_names_only_what_differs_from_a_default_launch() {
        let ctx = context();
        let zen = app("Zen", false, true);
        let defaults = ctx.selection(&zen, &LaunchMemory::default(), "de_DE.UTF-8");
        assert!(ctx.link_query(&zen, &defaults, "de_DE.UTF-8").is_empty());
        let chosen = LaunchSelection {
            home_dir: "work".into(),
            gpu: "intel-gpu".into(),
            vpn: "mullvad".into(),
            vpn_endpoint: "nl".into(),
            language: "ja_JP.UTF-8".into(),
            wayland: false,
            room: true,
        };
        let query = ctx.link_query(&zen, &chosen, "de_DE.UTF-8");
        let expected: Vec<(&str, String)> = vec![
            ("home", "work".into()),
            ("gpu", "intel-gpu".into()),
            ("vpn", "mullvad".into()),
            ("vpn_endpoint", "nl".into()),
            ("room", "1".into()),
            ("lang", "ja_JP.UTF-8".into()),
            ("wayland", "0".into()),
        ];
        assert_eq!(query, expected);
    }

    #[test]
    fn auto_home_names_match_the_web_launcher() {
        assert_eq!(auto_home_name("Zen"), "auto-zen");
        assert_eq!(auto_home_name("VS Code"), "auto-vs-code");
        assert_eq!(auto_home_name("My  App_2"), "auto-my-app-2");
        assert_eq!(auto_home_name("Café!"), "auto-caf");
    }

    #[test]
    fn default_language_is_the_locale_nearest_the_macs() {
        assert_eq!(default_language("de-DE"), "de_DE.UTF-8");
        assert_eq!(default_language("en_US"), "en_US.UTF-8");
        assert_eq!(default_language("pt"), "pt_BR.UTF-8");
        assert_eq!(default_language("de-LI"), "de_DE.UTF-8");
        assert_eq!(default_language("zh-HK"), "zh_HK.UTF-8");
        assert_eq!(default_language("zh-SG"), "zh_SG.UTF-8");
        assert_eq!(default_language("nl-XX"), "nl_AW.UTF-8");
        assert_eq!(default_language("xx-YY"), DEFAULT_LANGUAGE);
        assert_eq!(default_language(""), DEFAULT_LANGUAGE);
    }

    #[test]
    fn languages_list_the_default_first_with_mac_names() {
        let options =
            language_options(|locale| (locale == "de_DE").then(|| "German (Germany)".into()));
        assert_eq!(options[0].id, DEFAULT_LANGUAGE);
        assert_eq!(options.len(), LOCALES.len() + 1);
        assert!(!LOCALES.contains(&DEFAULT_LANGUAGE));
        let german = options.iter().find(|o| o.id == "de_DE.UTF-8").unwrap();
        assert_eq!(german.label, "German (Germany) · de_DE");
        let unnamed = options.iter().find(|o| o.id == "fr_FR.UTF-8").unwrap();
        assert_eq!(unnamed.label, "fr_FR");
    }
}
