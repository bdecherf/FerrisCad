//! The AI assistant: builds a description of the circuit and asks an AI service about it.

use crate::checks::{Issue, PcbConn, Severity};
use crate::model::*;
use crate::sim::SimResult;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    /// The Claude Code app already installed and signed in on this computer.
    ClaudeCode,
    Claude,
    OpenAiCompatible,
    Offline,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiSettings {
    pub provider: Provider,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub remember_key: bool,
    /// Model for Claude Code ("" = its default, or sonnet / opus / haiku).
    #[serde(default)]
    pub cli_model: String,
    /// Let Claude Code read documentation (read-only) in the folders below.
    #[serde(default)]
    pub allow_files: bool,
    #[serde(default = "yes")]
    pub include_project: bool,
    #[serde(default)]
    pub doc_dirs: Vec<String>,
    /// Let Claude Code search the web (datasheets, exact library part names).
    #[serde(default)]
    pub allow_web: bool,
    /// Folder of the open project (set by the app, not saved).
    #[serde(skip)]
    pub project_dir: Option<PathBuf>,
}

fn yes() -> bool {
    true
}

impl Default for AiSettings {
    fn default() -> Self {
        AiSettings {
            provider: Provider::Claude,
            model: "claude-sonnet-5-5".into(),
            base_url: "http://localhost:11434/v1".into(),
            api_key: String::new(),
            remember_key: false,
            cli_model: String::new(),
            allow_files: false,
            include_project: true,
            doc_dirs: vec![],
            allow_web: false,
            project_dir: None,
        }
    }
}

/// The app's settings folder (~/.config/ferriscad on Linux).
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let dir = base.join("ferriscad");
    // The app used to be called FerrisCad: bring its settings and recent files along once.
    let old = base.join(["volt", "pilot"].concat());
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::create_dir_all(&dir);
        for f in ["settings.json", "recent.json"] {
            let _ = std::fs::copy(old.join(f), dir.join(f));
        }
    }
    Some(dir)
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
}

impl AiSettings {
    pub fn load() -> Self {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(Self::detect)
    }
    /// First start: pick whatever AI is already available on this computer.
    pub fn detect() -> Self {
        let mut s = AiSettings::default();
        if !s.effective_key().is_empty() {
            return s;
        }
        if find_claude_cli().is_some() {
            s.provider = Provider::ClaudeCode;
            return s;
        }
        if let Some(m) = ollama_models().into_iter().next() {
            s.provider = Provider::OpenAiCompatible;
            s.model = m;
        }
        s
    }
    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or("no settings folder")?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut copy = self.clone();
        if !copy.remember_key {
            copy.api_key.clear();
        }
        std::fs::write(path, serde_json::to_string_pretty(&copy).unwrap()).map_err(|e| e.to_string())
    }
    pub fn effective_key(&self) -> String {
        if !self.api_key.trim().is_empty() {
            return self.api_key.trim().to_string();
        }
        let var = match self.provider {
            Provider::Claude => "ANTHROPIC_API_KEY",
            _ => "OPENAI_API_KEY",
        };
        std::env::var(var).unwrap_or_default()
    }
    /// Whether questions can go to a real AI service.
    pub fn online(&self) -> bool {
        match self.provider {
            Provider::ClaudeCode => find_claude_cli().is_some(),
            Provider::Claude => !self.effective_key().is_empty(),
            Provider::OpenAiCompatible => !self.base_url.trim().is_empty(),
            Provider::Offline => false,
        }
    }
    pub fn web_enabled(&self) -> bool {
        self.allow_web && self.provider == Provider::ClaudeCode
    }
    /// Folders the AI may read (only with Claude Code, only when allowed).
    pub fn readable_dirs(&self) -> Vec<PathBuf> {
        if !self.allow_files || self.provider != Provider::ClaudeCode {
            return vec![];
        }
        let mut v: Vec<PathBuf> = vec![];
        if self.include_project {
            if let Some(p) = &self.project_dir {
                v.push(p.clone());
            }
        }
        for d in &self.doc_dirs {
            let p = PathBuf::from(d);
            if p.is_dir() && !v.contains(&p) {
                v.push(p);
            }
        }
        v
    }
    /// Text added to the system prompt telling the AI what it may read.
    pub fn files_note(&self) -> String {
        let web = if self.web_enabled() {
            "\n\nYou can search the internet with WebSearch and WebFetch: use them for datasheets, pinouts, typical application \
             circuits, and to find exact KiCad library names (browse https://gitlab.com/kicad/libraries/kicad-symbols/-/tree/master \
             where each library is a <Library>.kicad_symdir folder with one <Symbol>.kicad_sym file per part). Say which sources you used."
        } else {
            ""
        };
        let dirs = self.readable_dirs();
        if dirs.is_empty() {
            return web.to_string();
        }
        let list: Vec<String> = dirs.iter().map(|d| format!("- {}", d.display())).collect();
        format!(
            "\n\nYou can read files (read-only) in these folders with your Read, Glob and Grep tools:\n{}\n\
             Use them to look up datasheets, application notes and the project's own files when that helps. \
             Say which file and page you used. You cannot change files.{}",
            list.join("\n"),
            web
        )
    }
    pub fn describe(&self) -> String {
        match self.provider {
            Provider::ClaudeCode if self.online() => {
                let n = self.readable_dirs().len();
                format!(
                    "Claude Code on this computer{}{}",
                    if self.cli_model.is_empty() { String::new() } else { format!(" · {}", self.cli_model) },
                    if n > 0 { format!(" · can read {} folder{}", n, if n > 1 { "s" } else { "" }) } else { String::new() }
                ) + if self.web_enabled() { " · web" } else { "" }
            }
            Provider::Claude if self.online() => format!("Claude · {}", self.model),
            Provider::OpenAiCompatible => format!("{} · {}", short_host(&self.base_url), self.model),
            _ => "Built-in checker (no AI connected)".into(),
        }
    }
}

/// Where the Claude Code command is installed, if it is.
pub fn find_claude_cli() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(h) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let h = PathBuf::from(h);
        dirs.push(h.join(".local").join("bin"));
        dirs.push(h.join(".claude").join("local"));
        dirs.push(h.join(".npm-global").join("bin"));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// Models of a local Ollama server, if one is running.
pub fn ollama_models() -> Vec<String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_millis(800)).build();
    agent
        .get("http://localhost:11434/api/tags")
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect()))
        .unwrap_or_default()
}

/// Ask through the Claude Code command: it uses the sign-in already on this computer.
/// Its tools are off, except read-only file tools limited to the folders the person allowed.
fn ask_claude_code(settings: &AiSettings, system: &str, history: &History) -> Result<String, String> {
    use std::io::Write;
    let cli = find_claude_cli().ok_or("Claude Code isn't installed on this computer (https://claude.com/claude-code).")?;
    let dir = std::env::temp_dir().join("ferriscad-ai");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let sys_file = dir.join("system.txt");
    std::fs::write(&sys_file, system).map_err(|e| e.to_string())?;
    let mut prompt = String::new();
    if history.len() > 1 {
        prompt.push_str("Conversation so far:\n\n");
        for (role, text) in &history[..history.len() - 1] {
            prompt.push_str(&format!("[{}]\n{}\n\n", if role == "user" { "User" } else { "Assistant" }, text));
        }
        prompt.push_str("Now answer the user's latest message:\n\n");
    }
    if let Some((_, last)) = history.last() {
        prompt.push_str(last);
    }
    let readable = settings.readable_dirs();
    let mut cmd = std::process::Command::new(&cli);
    cmd.args(["-p", "--output-format", "text"]);
    let mut tools: Vec<&str> = vec![];
    if !readable.is_empty() {
        tools.extend(["Read", "Glob", "Grep"]);
    }
    if settings.web_enabled() {
        tools.extend(["WebSearch", "WebFetch"]);
    }
    if tools.is_empty() {
        cmd.args(["--tools", ""]).current_dir(&dir);
    } else {
        // Only the allowed tools (read-only files, web lookups); everything else is refused.
        let list = tools.join(",");
        cmd.args(["--tools", &list, "--allowedTools", &list, "--permission-mode", "dontAsk"]);
        for d in &readable {
            cmd.arg("--add-dir").arg(d);
        }
        cmd.current_dir(readable.first().unwrap_or(&dir));
    }
    cmd.arg("--system-prompt-file")
        .arg(&sys_file)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if !settings.cli_model.trim().is_empty() {
        cmd.arg("--model").arg(settings.cli_model.trim());
    }
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't start Claude Code: {}", e))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !text.is_empty() {
        return Ok(text);
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let msg = if err.is_empty() { text } else { err };
    let low = msg.to_lowercase();
    if low.contains("login") || low.contains("log in") || low.contains("auth") {
        Err(format!("Claude Code isn't signed in. Open a terminal, run `claude` once and sign in. ({})", msg))
    } else {
        Err(format!("Claude Code answered with an error: {}", msg.chars().take(400).collect::<String>()))
    }
}

fn short_host(u: &str) -> String {
    u.trim_start_matches("https://").trim_start_matches("http://").split('/').next().unwrap_or(u).to_string()
}

pub const SYSTEM_INTRO: &str = "You are the electronics assistant built into FerrisCad, a schematic and PCB layout editor. \
You are an experienced hardware engineer helping the user design and debug their circuit. \
Below you get the user's whole circuit: parts, nets (which pins are connected together), the results of the automatic \
electrical rules check (ERC), board design rules check (DRC), and a DC operating-point simulation. \
Explain problems in plain language a beginner understands, refer to parts by their reference (R1, U2...) and to nets by name, \
and be concrete: values, orientation, missing or wrong connections, what to measure with a multimeter. \
Keep answers short and focused, using short bullet points.

You CAN change the design. Write each change on its own line inside double square brackets; FerrisCad turns them into \
buttons the user applies with one click (or all at once, and can undo with Ctrl+Z). Explain the change before the lines. \
Only propose changes the user asked for or that fix a clear problem. Available changes:
[[SET R1 value=330]]                  change a value (also ref=NEWNAME, or lcsc=C17414 for the JLCPCB/LCSC part number used for assembly)
[[ADD kicad=Regulator_Linear:L7805 ref=U3 near=U1]]   add an exact part from the official KiCad library (FerrisCad downloads its symbol and footprint). Use real library names (Device, Regulator_Linear, Amplifier_Operational, Timer, Transistor_BJT, Transistor_FET, MCU_Microchip_ATmega, MCU_ST_STM32F1, Interface_USB, Sensor_Temperature, Connector_Generic...) and symbol names as in KiCad (e.g. LM7805_TO220, NE555P, STM32F103C8Tx)
[[ADD resistor ref=R7 value=10k near=U2]]   add a simple built-in part; kinds: resistor, capacitor, polarized_capacitor, inductor, diode, led, npn, pnp, nmos, opamp, mcu, regulator, battery, switch, connector, gnd, power. Position with near=REF or at=x,y (schematic units, grid 10); optional rot=90
[[CONNECT R7.1 U2.IO0]]               wire two pins together (pin = REF.PIN, PIN is the pin name or number)
[[CONNECT R7.2 GND]]                  connect a pin to a supply (GND, +5V, +3V3...) or to a named net
[[DELETE R7]]                         remove a part
[[MOVE R7 x,y]]                       move a part on the schematic
[[ROTATE R7]]                         rotate a part a quarter turn on the schematic
[[PLACE R7 x,y rot=90]]               place a footprint on the board (millimetres; the board outline is given below)
[[ROUTE R7.1 U2.IO0 layer=F width=0.4]]  draw a copper track between two pads (layer F = front, B = back, In1, In2... = inner layers)
[[LAYERS 4]]                          set the number of copper layers (2, 4, 6...)
[[AUTOPLACE]]                         arrange all footprints inside the board outline
For a new part, ADD it first, then CONNECT its pins, then PLACE it on the board. For a library part you add in the same \
answer you don't know its exact pin names yet: refer to its pins by number (e.g. U3.4).";

/// Plain-text description of the circuit for the AI.
pub fn circuit_report(p: &Project, nl: &Netlist, issues: &[Issue], sim: &SimResult, pc: &PcbConn) -> String {
    let mut s = String::new();
    s.push_str(&format!("PROJECT: {}\n\nPARTS:\n", p.name));
    let mut comps: Vec<(usize, &Component)> =
        p.components.iter().enumerate().filter(|(_, c)| !c.kind.is_power() && !c.info().pins.is_empty()).collect();
    comps.sort_by(|a, b| natural_key(&a.1.reference).cmp(&natural_key(&b.1.reference)));
    for (ci, c) in &comps {
        let inf = c.info();
        let pins: Vec<String> = inf
            .pins
            .iter()
            .enumerate()
            .map(|(pi, pd)| {
                let net = nl.net_of(*ci, pi).map(|n| nl.nets[n].name.clone()).unwrap_or_else(|| "?".into());
                format!("{}->{}", pd.name, net)
            })
            .collect();
        s.push_str(&format!(
            "- {} {} value={} footprint={} pins: {}",
            c.reference,
            inf.label,
            c.value,
            inf.footprint,
            pins.join(", ")
        ));
        if let Some(n) = sim.notes.get(&c.id) {
            s.push_str(&format!(" | DC: {}", n));
        }
        if !c.lcsc.is_empty() {
            s.push_str(&format!(" | JLCPCB part {}", c.lcsc));
        }
        s.push_str(&format!(" | schematic at {},{} rot {}", c.pos.0, c.pos.1, c.rot as u32 * 90));
        if c.has_fp() {
            let b = c.pcb_bbox();
            s.push_str(&format!(
                " | board at {:.2},{:.2} mm rot {} size {:.1}x{:.1} mm",
                c.pcb_pos.0,
                c.pcb_pos.1,
                c.pcb_rot as u32 * 90,
                b.2 - b.0,
                b.3 - b.1
            ));
        }
        s.push('\n');
    }
    s.push_str("\nNETS:\n");
    for (ni, net) in nl.nets.iter().enumerate() {
        let pins: Vec<String> = nl
            .part_pins(p, ni)
            .map(|(ci, pi)| format!("{}.{}", p.components[ci].reference, p.components[ci].info().pins[pi].name))
            .collect();
        if pins.is_empty() && net.power.is_empty() {
            continue;
        }
        let volt = sim.net_v.get(ni).copied().flatten().map(|v| format!(" ({})", fmt_si(v, "V"))).unwrap_or_default();
        s.push_str(&format!("- {}{}: {}\n", net.name, volt, if pins.is_empty() { "(no pins)".into() } else { pins.join(", ") }));
    }
    s.push_str("\nCHECK RESULTS:\n");
    if issues.is_empty() {
        s.push_str("- no problems found\n");
    }
    for is in issues {
        s.push_str(&format!("- [{:?}][{}] {}\n", is.severity, is.source, is.message));
    }
    s.push_str(&format!("\nDC SIMULATION: {}\n", sim.message));
    for a in &sim.assumed {
        s.push_str(&format!("- note: {}\n", a));
    }
    let b = &p.board;
    s.push_str(&format!(
        "\nBOARD: {} copper layers, outline from {:.1},{:.1} to {:.1},{:.1} mm ({:.1} x {:.1} mm), {} track segments, {} vias, {} connection(s) still unrouted.\n",
        b.layers,
        b.x0,
        b.y0,
        b.x1,
        b.y1,
        b.x1 - b.x0,
        b.y1 - b.y0,
        p.tracks.len(),
        p.vias.len(),
        pc.unrouted
    ));
    s
}

pub fn system_prompt(report: &str) -> String {
    format!("{}\n\n=== CURRENT CIRCUIT ===\n{}", SYSTEM_INTRO, report)
}

/// Answer from the built-in checker when no AI service is connected.
pub fn offline_answer(question: &str, p: &Project, issues: &[Issue], sim: &SimResult) -> String {
    let mut s = String::new();
    s.push_str("No AI service is connected yet, so here is what the built-in checker sees");
    if !question.trim().is_empty() {
        s.push_str(" (click \"Connect an AI…\" for a real conversation)");
    }
    s.push_str(":\n");
    let errs: Vec<&Issue> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
    let warns: Vec<&Issue> = issues.iter().filter(|i| i.severity == Severity::Warning && i.source != "DRC").collect();
    let infos: Vec<&Issue> = issues.iter().filter(|i| i.severity == Severity::Info && i.source == "SIM").collect();
    if errs.is_empty() && warns.is_empty() {
        s.push_str("\n- The schematic passes all electrical checks.\n");
    }
    if !errs.is_empty() {
        s.push_str("\n# Must fix\n");
        for i in &errs {
            s.push_str(&format!("- {}\n", i.message));
        }
    }
    if !warns.is_empty() {
        s.push_str("\n# Worth checking\n");
        for i in warns.iter().take(8) {
            s.push_str(&format!("- {}\n", i.message));
        }
    }
    if !infos.is_empty() {
        s.push_str("\n# Good to know\n");
        for i in &infos {
            s.push_str(&format!("- {}\n", i.message));
        }
        for a in &sim.assumed {
            s.push_str(&format!("- {}\n", a));
        }
    }
    let unrouted: usize = issues.iter().filter(|i| i.source == "DRC" && i.message.contains("not routed")).count();
    if unrouted > 0 {
        s.push_str(&format!("\n- On the board, {} net(s) still have unrouted connections (thin lines in the layout view).\n", unrouted));
    }
    for i in issues {
        if let Some((cid, val)) = &i.fix {
            if let Some(ci) = p.find(*cid) {
                s.push_str(&format!("\n[[SET {} value={}]]", p.components[ci].reference, val));
            }
        }
    }
    s
}

pub type History = Vec<(String, String)>;

/// Blocking call to the configured AI service.
pub fn ask(settings: &AiSettings, system: &str, history: &History) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(180)).build();
    let key = settings.effective_key();
    match settings.provider {
        Provider::Offline => Err("offline".into()),
        Provider::ClaudeCode => ask_claude_code(settings, system, history),
        Provider::Claude => {
            let msgs: Vec<serde_json::Value> =
                history.iter().map(|(r, t)| serde_json::json!({"role": r, "content": t})).collect();
            let body = serde_json::json!({
                "model": settings.model,
                "max_tokens": 2048,
                "system": system,
                "messages": msgs,
            });
            let resp = agent
                .post("https://api.anthropic.com/v1/messages")
                .set("x-api-key", &key)
                .set("anthropic-version", "2023-06-01")
                .set("content-type", "application/json")
                .send_string(&body.to_string());
            let text = read(resp)?;
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let out: String = v["content"]
                .as_array()
                .map(|a| a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            if out.is_empty() {
                Err(format!("Unexpected answer: {}", text.chars().take(300).collect::<String>()))
            } else {
                Ok(out)
            }
        }
        Provider::OpenAiCompatible => {
            let mut msgs = vec![serde_json::json!({"role": "system", "content": system})];
            msgs.extend(history.iter().map(|(r, t)| serde_json::json!({"role": r, "content": t})));
            let body = serde_json::json!({ "model": settings.model, "messages": msgs });
            let url = format!("{}/chat/completions", settings.base_url.trim_end_matches('/'));
            let mut req = agent.post(&url).set("content-type", "application/json");
            if !key.is_empty() {
                req = req.set("authorization", &format!("Bearer {}", key));
            }
            let text = read(req.send_string(&body.to_string()))?;
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            v["choices"][0]["message"]["content"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| format!("Unexpected answer: {}", text.chars().take(300).collect::<String>()))
        }
    }
}

fn read(r: Result<ureq::Response, ureq::Error>) -> Result<String, String> {
    match r {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
                .unwrap_or(body);
            Err(format!("The AI service answered with error {}: {}", code, msg))
        }
        Err(e) => Err(format!("Couldn't reach the AI service: {}", e)),
    }
}

/// Pull out [[SET R1 value=330]] actions; returns the cleaned text and the actions.
/// One design change proposed by the AI, e.g. `[[ADD resistor ref=R7 value=10k near=R1]]`.
#[derive(Clone, Debug)]
pub struct AiAction {
    pub verb: String,
    pub args: Vec<String>,
    pub kv: Vec<(String, String)>,
}

pub const VERBS: [&str; 10] = ["SET", "ADD", "CONNECT", "DELETE", "MOVE", "PLACE", "ROUTE", "AUTOPLACE", "ROTATE", "LAYERS"];

impl AiAction {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.kv.iter().find(|(a, _)| a.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
    pub fn arg(&self, i: usize) -> &str {
        self.args.get(i).map(|s| s.as_str()).unwrap_or("")
    }
    /// Short description for the button.
    pub fn label(&self) -> String {
        match self.verb.as_str() {
            "SET" => {
                let mut v = vec![];
                if let Some(x) = self.get("value") {
                    v.push(format!("{} = {}", self.arg(0), x));
                }
                if let Some(x) = self.get("ref") {
                    v.push(format!("rename {} to {}", self.arg(0), x));
                }
                if v.is_empty() && !self.arg(1).is_empty() {
                    v.push(format!("{} = {}", self.arg(0), self.arg(1)));
                }
                v.join(", ")
            }
            "ADD" => format!(
                "Add {}{}{}",
                self.arg(0).to_lowercase().replace('_', " "),
                self.get("ref").map(|r| format!(" {}", r)).unwrap_or_default(),
                self.get("value").map(|v| format!(" ({})", v)).unwrap_or_default()
            ),
            "CONNECT" => format!("Connect {} to {}", self.arg(0), self.arg(1)),
            "DELETE" => format!("Remove {}", self.arg(0)),
            "MOVE" => format!("Move {} on the schematic", self.arg(0)),
            "PLACE" => format!("Place {} on the board at {}", self.arg(0), self.arg(1)),
            "ROUTE" => format!("Route a track {} to {}", self.arg(0), self.arg(1)),
            "ROTATE" => format!("Rotate {}", self.arg(0)),
            "AUTOPLACE" => "Arrange the parts on the board".into(),
            "LAYERS" => format!("Use {} copper layers", self.arg(0)),
            v => v.to_string(),
        }
    }
}

/// Pull out `[[VERB ...]]` actions; returns the cleaned text and the actions.
pub fn parse_actions(text: &str) -> (String, Vec<AiAction>) {
    let mut out = String::new();
    let mut acts = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("[[") {
        let after = &rest[i + 2..];
        let Some(j) = after.find("]]") else { break };
        let inner = after[..j].trim();
        let mut toks = inner.split_whitespace();
        let verb = toks.next().unwrap_or("").to_uppercase();
        // Only a change written on its own line counts; a mention inside a sentence is just text.
        let line_start = rest[..i].rfind('\n').map(|k| k + 1).unwrap_or(0);
        let before = rest[line_start..i].trim().trim_start_matches(['-', '*', '•']).trim();
        let after_line = after[j + 2..].split('\n').next().unwrap_or("").trim();
        let own_line = before.is_empty() && (after_line.is_empty() || after_line.starts_with("[["));
        if VERBS.contains(&verb.as_str()) && own_line {
            out.push_str(&rest[..i]);
            let mut a = AiAction { verb, args: vec![], kv: vec![] };
            for t in toks {
                match t.split_once('=') {
                    Some((k, v)) if !k.is_empty() => a.kv.push((k.to_string(), v.trim_matches('"').to_string())),
                    _ => a.args.push(t.trim_matches('"').to_string()),
                }
            }
            acts.push(a);
        } else {
            out.push_str(&rest[..i + 2 + j + 2]);
        }
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    (out.trim().to_string(), acts)
}

/// Part references mentioned in the text that exist in the project.
pub fn mentioned_refs(text: &str, p: &Project) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for tok in text.split(|c: char| !c.is_ascii_alphanumeric()) {
        if tok.len() < 2 || !tok.chars().last().map_or(false, |c| c.is_ascii_digit()) {
            continue;
        }
        if let Some(ci) = p.find_ref(tok) {
            let r = p.components[ci].reference.clone();
            if !v.contains(&r) {
                v.push(r);
            }
        }
    }
    v
}
