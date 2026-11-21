//! 动画包：pets/<名字>/ 下的一组动画。每段动画可以是
//!   - 精灵图 PNG（帧按网格排，pet.json 里写 frames / columns）
//!   - GIF 文件（帧数、每帧时长从 GIF 里读）
//!   - 一个放着 PNG 帧的文件夹（001.png、002.png…，按文件名的自然顺序）
//! 有 pet.json 就按它来；没有就按文件名自动认（见 auto_pack），放几个 GIF 进去就是一个宠物。
//!
//! 内存策略：只解码正在播的几段动画（App 里有个小 LRU）。调色板 PNG、以及总共不超过 256 色的
//! GIF / 帧文件夹按「每像素 1 字节」存，渲染时查表；其余每像素 4 字节。缩放在每帧渲染时做，不预先放大。

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

/// GIF / 帧文件夹最多读这么多帧，防止误放一个超长动图把内存吃光
const MAX_FRAMES: usize = 2000;

#[derive(Deserialize, Clone)]
pub struct AnimDef {
    pub file: String,
    /// 精灵图必填；GIF 和帧文件夹不用写，自己数
    #[serde(default)]
    pub frames: usize,
    #[serde(default)]
    pub columns: usize,
    #[serde(default = "default_fps")]
    pub fps: f32,
    /// 可选：逐帧时长（毫秒）。写了就覆盖 fps，不够长的部分沿用最后一个值
    #[serde(default)]
    pub durations: Vec<u32>,
}

fn default_fps() -> f32 {
    12.0
}

#[derive(Deserialize, Clone)]
pub struct MenuGroup {
    pub group: String,
    pub items: Vec<String>,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct IdleExtras {
    pub min_secs: u64,
    pub max_secs: u64,
    pub pool: Vec<String>,
}

impl Default for IdleExtras {
    fn default() -> Self {
        Self { min_secs: 30, max_secs: 90, pool: vec![] }
    }
}

#[derive(Deserialize)]
struct PackFile {
    #[serde(default)]
    name: String,
    #[serde(default = "one")]
    scale: f32,
    animations: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    states: HashMap<String, Vec<String>>,
    #[serde(default)]
    idle_extras: IdleExtras,
    #[serde(default)]
    menu: Vec<MenuGroup>,
    /// 像素画：缩放时用最近邻，保持像素边缘锐利
    #[serde(default)]
    pixelated: bool,
}

fn one() -> f32 {
    1.0
}

pub struct Pack {
    pub dir: PathBuf,
    pub name: String,
    pub scale: f32,
    /// 按 pet.json 里的顺序
    pub anims: Vec<(String, AnimDef)>,
    pub states: HashMap<String, Vec<String>>,
    pub idle_extras: IdleExtras,
    pub menu: Vec<MenuGroup>,
    pub pixelated: bool,
}

/// pets/ 下所有可用的动画包：(文件夹名, 显示名)
pub fn list_packs(pets_dir: &Path) -> Vec<(String, String)> {
    #[derive(Deserialize)]
    struct Head {
        #[serde(default)]
        name: String,
    }
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(pets_dir) else { return out };
    for e in rd.flatten() {
        let path = e.path();
        if !path.is_dir() {
            continue;
        }
        let dir = e.file_name().to_string_lossy().into_owned();
        let name = match std::fs::read_to_string(path.join("pet.json")) {
            Ok(text) => serde_json::from_str::<Head>(text.trim_start_matches('\u{feff}'))
                .ok()
                .map(|h| h.name)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| dir.clone()),
            // 没有 pet.json：里面有 GIF 或帧文件夹才算一个宠物
            Err(_) if !auto_sources(&path).is_empty() => dir.clone(),
            Err(_) => continue,
        };
        out.push((dir, name));
    }
    out.sort();
    out
}

impl Pack {
    pub fn load(dir: &Path) -> Result<Pack, String> {
        let path = dir.join("pet.json");
        if !path.is_file() {
            return auto_pack(dir);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("读不到 {}：{e}", path.display()))?;
        let pf: PackFile = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("pet.json 格式错误：{e}"))?;
        let mut anims = Vec::new();
        for (name, v) in pf.animations {
            let def: AnimDef = serde_json::from_value(v).map_err(|e| format!("动画「{name}」配置错误：{e}"))?;
            if def.frames == 0 && !is_auto_source(&dir.join(&def.file)) {
                return Err(format!("动画「{name}」frames 不能为 0（只有 GIF 和帧文件夹可以不写 frames）"));
            }
            anims.push((name, def));
        }
        if anims.is_empty() {
            return Err("pet.json 里没有任何动画".into());
        }
        let has = |n: &String| anims.iter().any(|(a, _)| a == n);
        let states = pf
            .states
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().filter(|n| has(n)).collect::<Vec<_>>()))
            .collect();
        let mut idle_extras = pf.idle_extras;
        idle_extras.pool.retain(|n| has(n));
        let mut menu: Vec<MenuGroup> = pf
            .menu
            .into_iter()
            .map(|g| MenuGroup { group: g.group, items: g.items.into_iter().filter(|n| has(n)).collect() })
            .filter(|g| !g.items.is_empty())
            .collect();
        if menu.is_empty() {
            menu.push(MenuGroup { group: "全部动作".into(), items: anims.iter().map(|(n, _)| n.clone()).collect() });
        }
        Ok(Pack {
            dir: dir.to_path_buf(),
            name: if pf.name.is_empty() { dir_name(dir) } else { pf.name },
            scale: if pf.scale > 0.0 { pf.scale } else { 1.0 },
            anims,
            states,
            idle_extras,
            menu,
            pixelated: pf.pixelated,
        })
    }

    pub fn def(&self, name: &str) -> Option<&AnimDef> {
        self.anims.iter().find(|(n, _)| n == name).map(|(_, d)| d)
    }

    pub fn load_sheet(&self, name: &str) -> Result<Sheet, String> {
        let def = self.def(name).ok_or_else(|| format!("没有动画「{name}」"))?;
        Sheet::load(&self.dir.join(&def.file), def, self.pixelated)
    }
}

fn dir_name(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension().map(|e| e.eq_ignore_ascii_case(ext)).unwrap_or(false)
}

/// 帧数由程序自己数的来源：GIF 文件，或放 PNG 帧的文件夹
fn is_auto_source(path: &Path) -> bool {
    path.is_dir() || has_ext(path, "gif")
}

/// 自然排序用的键：数字段补齐到 12 位，让 2.png 排在 10.png 前面
fn natural_key(s: &str) -> String {
    let mut out = String::new();
    let mut digits = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if !digits.is_empty() {
            out.push_str(&format!("{digits:0>12}"));
            digits.clear();
        }
        out.push(c);
    }
    if !digits.is_empty() {
        out.push_str(&format!("{digits:0>12}"));
    }
    out
}

/// 帧文件夹里的 PNG，按文件名自然顺序
fn frame_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && has_ext(p, "png")).collect();
    files.sort_by_cached_key(|p| natural_key(&p.file_name().unwrap_or_default().to_string_lossy()));
    files
}

/// 没有 pet.json 时能当动画用的东西：(动画名, 相对路径)。
/// 在宠物文件夹和它的 anims/ 子文件夹里找 *.gif 和装着 PNG 帧的子文件夹，动画名取文件名（去掉扩展名）。
fn auto_sources(dir: &Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (base, prefix) in [(dir.to_path_buf(), ""), (dir.join("anims"), "anims/")] {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let fname = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if prefix.is_empty() && fname.eq_ignore_ascii_case("anims") {
                    continue;
                }
                if !frame_files(&p).is_empty() {
                    out.push((fname.clone(), format!("{prefix}{fname}")));
                }
            } else if has_ext(&p, "gif") {
                let stem = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                out.push((stem, format!("{prefix}{fname}")));
            }
        }
    }
    out.sort_by_cached_key(|(n, _)| natural_key(n));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// 文件名 → 状态。英文别名要整词匹配（等于它，或后面跟着 - _ 空格 数字 等非字母），
/// 例如 click-2.gif、idle_1.gif；中文别名只要开头对上，例如「待机2」「工作中」。
/// 长的放前面：drag_left 要比 drag 先判断。
const STATE_ALIASES: [(&str, &[&str]); 11] = [
    ("drag_left", &["drag_left", "drag-left", "向左"]),
    ("drag_right", &["drag_right", "drag-right", "向右"]),
    ("idle", &["idle", "待机"]),
    ("thinking", &["thinking", "think", "思考"]),
    ("working", &["working", "work", "工作"]),
    ("result", &["result", "整理"]),
    ("waiting", &["waiting", "wait", "等待"]),
    ("success", &["success", "done", "完成"]),
    ("error", &["error", "failed", "fail", "出错", "失败"]),
    ("drag", &["drag", "拖动", "拖拽"]),
    ("click", &["click", "点击"]),
];

fn state_of(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    for (key, aliases) in STATE_ALIASES {
        for a in aliases {
            let Some(rest) = n.strip_prefix(a) else { continue };
            let cjk = !a.is_ascii();
            if cjk || rest.chars().next().map(|c| !c.is_ascii_alphabetic()).unwrap_or(true) {
                return Some(key);
            }
        }
    }
    None
}

/// GIF / 帧文件夹 / PNG 的画面尺寸，只读文件头
fn source_size(path: &Path) -> Option<(usize, usize)> {
    if has_ext(path, "gif") {
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let d = opts.read_info(BufReader::new(File::open(path).ok()?)).ok()?;
        return Some((d.width() as usize, d.height() as usize));
    }
    let png_path = if path.is_dir() { frame_files(path).into_iter().next()? } else { path.to_path_buf() };
    let r = png::Decoder::new(BufReader::new(File::open(png_path).ok()?)).read_info().ok()?;
    Some((r.info().width as usize, r.info().height as usize))
}

/// 没有 pet.json 的宠物：按文件名分配状态，认不出来的当待机小动作；没有待机就拿第一个动画顶上。
/// 默认大小按待机动画的高度定（显示成 160 像素左右），小于 96 像素高的按像素画处理。
fn auto_pack(dir: &Path) -> Result<Pack, String> {
    let sources = auto_sources(dir);
    if sources.is_empty() {
        return Err(format!("{} 里没有 pet.json，也没有 GIF 或 PNG 帧文件夹", dir.display()));
    }
    let mut states: HashMap<String, Vec<String>> = HashMap::new();
    let mut extras = Vec::new();
    let mut anims = Vec::new();
    for (name, file) in sources {
        match state_of(&name) {
            Some(key) => states.entry(key.to_string()).or_default().push(name.clone()),
            None => extras.push(name.clone()),
        }
        anims.push((name, AnimDef { file, frames: 0, columns: 0, fps: default_fps(), durations: Vec::new() }));
    }
    if !states.contains_key("idle") {
        let first = extras.first().cloned().unwrap_or_else(|| anims[0].0.clone());
        extras.retain(|n| *n != first);
        states.insert("idle".into(), vec![first]);
    }
    let idle = &states["idle"][0];
    let idle_file = anims.iter().find(|(n, _)| n == idle).map(|(_, d)| dir.join(&d.file));
    let (scale, pixelated) = match idle_file.and_then(|p| source_size(&p)) {
        Some((_, h)) if h > 0 => ((((160.0 / h as f32).clamp(0.5, 3.0)) * 4.0).round() / 4.0, h <= 96),
        _ => (1.0, false),
    };
    let menu = vec![MenuGroup { group: "全部动作".into(), items: anims.iter().map(|(n, _)| n.clone()).collect() }];
    Ok(Pack {
        dir: dir.to_path_buf(),
        name: dir_name(dir),
        scale,
        anims,
        states,
        idle_extras: IdleExtras { pool: extras, ..Default::default() },
        menu,
        pixelated,
    })
}

enum Pixels {
    /// 每像素 1 字节调色板索引 + 预乘好的 BGRA 查找表
    Indexed { idx: Vec<u8>, lut: Box<[u32; 256]> },
    /// 每像素一个预乘 BGRA
    Bgra(Vec<u32>),
}

/// 每帧时长：pet.json 写了 durations 就用它（不够长的沿用最后一个），
/// 否则用 GIF 自带的时长，再否则按 fps
fn frame_durations(def: &AnimDef, n: usize, natural: Option<&[u32]>) -> Vec<u32> {
    let fps_ms = (1000.0 / def.fps.clamp(1.0, 60.0)).round() as u32;
    (0..n)
        .map(|i| {
            def.durations
                .get(i)
                .or(def.durations.last())
                .or_else(|| natural.and_then(|d| d.get(i)))
                .copied()
                .unwrap_or(fps_ms)
                .clamp(16, 10_000)
        })
        .collect()
}

/// 读 PNG 成预乘 BGRA：(宽, 高, 像素)
fn decode_png_premul(path: &Path) -> Result<(usize, usize, Vec<u32>), String> {
    let err = |e: png::DecodingError| format!("{} 解码失败：{e}", path.display());
    let file = File::open(path).map_err(|e| format!("打不开 {}：{e}", path.display()))?;
    let mut dec = png::Decoder::new(BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(err)?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let out = reader.next_frame(&mut buf).map_err(err)?;
    let (w, h) = (out.width as usize, out.height as usize);
    let ch = match out.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        _ => 1,
    };
    let mut v = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = &buf[y * out.line_size..];
        for x in 0..w {
            let p = &row[x * ch..x * ch + ch];
            v.push(match ch {
                4 => premul(p[0], p[1], p[2], p[3]),
                3 => premul(p[0], p[1], p[2], 255),
                2 => premul(p[0], p[0], p[0], p[1]),
                _ => premul(p[0], p[0], p[0], 255),
            });
        }
    }
    Ok((w, h, v))
}

/// 一帧帧往下拼。总共不超过 256 种颜色时存成调色板（每像素 1 字节），超了就转成 BGRA。
#[derive(Default)]
struct Strip {
    map: HashMap<u32, u8>,
    lut: Vec<u32>,
    idx: Vec<u8>,
    bgra: Option<Vec<u32>>,
    frames: usize,
}

impl Strip {
    fn push(&mut self, frame: &[u32]) {
        self.frames += 1;
        if self.bgra.is_none() {
            let mut fresh: HashSet<u32> = HashSet::new();
            for &p in frame {
                if !self.map.contains_key(&p) && fresh.insert(p) && self.map.len() + fresh.len() > 256 {
                    break;
                }
            }
            if self.map.len() + fresh.len() <= 256 {
                for p in fresh {
                    self.map.insert(p, self.lut.len() as u8);
                    self.lut.push(p);
                }
                let map = &self.map;
                self.idx.extend(frame.iter().map(|p| map[p]));
                return;
            }
            let lut = &self.lut;
            self.bgra = Some(self.idx.iter().map(|&i| lut[i as usize]).collect());
            self.idx = Vec::new();
        }
        if let Some(v) = self.bgra.as_mut() {
            v.extend_from_slice(frame);
        }
    }

    fn finish(self) -> Pixels {
        match self.bgra {
            Some(v) => Pixels::Bgra(v),
            None => {
                let mut lut = Box::new([0u32; 256]);
                lut[..self.lut.len()].copy_from_slice(&self.lut);
                Pixels::Indexed { idx: self.idx, lut }
            }
        }
    }
}

/// 解码 GIF，按 disposal 规则把每帧合成成完整画面。返回 (宽, 高, 帧, 每帧毫秒)
fn decode_gif(path: &Path) -> Result<(usize, usize, Strip, Option<Vec<u32>>), String> {
    use gif::DisposalMethod;
    let err = |e: gif::DecodingError| format!("{} 解码失败：{e}", path.display());
    let file = File::open(path).map_err(|e| format!("打不开 {}：{e}", path.display()))?;
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = opts.read_info(BufReader::new(file)).map_err(err)?;
    let (w, h) = (dec.width() as usize, dec.height() as usize);
    let mut canvas = vec![0u32; w * h];
    let mut strip = Strip::default();
    let mut delays = Vec::new();
    // 上一帧显示完以后要做的清理：(方式, 区域, 画这一帧之前的画面)
    let mut pending: Option<(DisposalMethod, (usize, usize, usize, usize), Option<Vec<u32>>)> = None;
    while let Some(f) = dec.read_next_frame().map_err(err)? {
        match pending.take() {
            Some((DisposalMethod::Background, (x0, y0, fw, fh), _)) => {
                for y in y0..(y0 + fh).min(h) {
                    for x in x0..(x0 + fw).min(w) {
                        canvas[y * w + x] = 0;
                    }
                }
            }
            Some((DisposalMethod::Previous, _, Some(saved))) => canvas = saved,
            _ => {}
        }
        let rect = (f.left as usize, f.top as usize, f.width as usize, f.height as usize);
        let saved = (f.dispose == DisposalMethod::Previous).then(|| canvas.clone());
        for row in 0..rect.3 {
            let y = rect.1 + row;
            if y >= h {
                break;
            }
            for col in 0..rect.2 {
                let x = rect.0 + col;
                if x >= w {
                    break;
                }
                let Some(p) = f.buffer.get((row * rect.2 + col) * 4..(row * rect.2 + col) * 4 + 4) else { continue };
                if p[3] != 0 {
                    canvas[y * w + x] = premul(p[0], p[1], p[2], p[3]);
                }
            }
        }
        strip.push(&canvas);
        // GIF 时长单位是 1/100 秒；0 或 1 按浏览器的习惯当 100 毫秒
        let d = f.delay as u32 * 10;
        delays.push(if d <= 10 { 100 } else { d });
        pending = Some((f.dispose, rect, saved));
        if delays.len() >= MAX_FRAMES {
            break;
        }
    }
    Ok((w, h, strip, Some(delays)))
}

/// 帧文件夹：每个 PNG 一帧，尺寸必须一样
fn decode_frame_dir(dir: &Path) -> Result<(usize, usize, Strip, Option<Vec<u32>>), String> {
    let files = frame_files(dir);
    if files.is_empty() {
        return Err(format!("文件夹 {} 里没有 PNG 帧", dir.display()));
    }
    let mut strip = Strip::default();
    let (mut w, mut h) = (0, 0);
    for (i, f) in files.iter().take(MAX_FRAMES).enumerate() {
        let (fw, fh, px) = decode_png_premul(f)?;
        if i == 0 {
            (w, h) = (fw, fh);
        } else if (fw, fh) != (w, h) {
            return Err(format!("{} 是 {fw}x{fh}，和第一帧的 {w}x{h} 不一样；帧文件夹里的图片尺寸要一致", f.display()));
        }
        strip.push(&px);
    }
    Ok((w, h, strip, None))
}

pub struct Sheet {
    px: Pixels,
    stride: usize,
    columns: usize,
    pub frames: usize,
    pub fw: usize,
    pub fh: usize,
    frame_ms: Vec<u32>,
    nearest: bool,
}

#[inline]
fn premul(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let m = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u32;
    m(b) | (m(g) << 8) | (m(r) << 16) | ((a as u32) << 24)
}

impl Sheet {
    fn load(path: &Path, def: &AnimDef, nearest: bool) -> Result<Sheet, String> {
        if is_auto_source(path) {
            return Sheet::load_frames(path, def, nearest);
        }
        let open = || File::open(path).map(BufReader::new).map_err(|e| format!("打不开 {}：{e}", path.display()));
        let err = |e: png::DecodingError| format!("{} 解码失败：{e}", path.display());

        // 先按原样读：8 位调色板图直接留索引
        let mut dec = png::Decoder::new(open()?);
        dec.set_transformations(png::Transformations::IDENTITY);
        let mut reader = dec.read_info().map_err(err)?;
        let info = reader.info();
        let (w, h) = (info.width as usize, info.height as usize);
        let px = if info.color_type == png::ColorType::Indexed && info.bit_depth == png::BitDepth::Eight {
            let pal = info.palette.as_ref().map(|p| p.to_vec()).unwrap_or_default();
            let trns = info.trns.as_ref().map(|t| t.to_vec()).unwrap_or_default();
            let mut lut = Box::new([0u32; 256]);
            for i in 0..256 {
                if i * 3 + 2 < pal.len() {
                    let a = trns.get(i).copied().unwrap_or(255);
                    lut[i] = premul(pal[i * 3], pal[i * 3 + 1], pal[i * 3 + 2], a);
                }
            }
            let mut buf = vec![0u8; reader.output_buffer_size()];
            let out = reader.next_frame(&mut buf).map_err(err)?;
            let mut idx = vec![0u8; w * h];
            for y in 0..h {
                idx[y * w..(y + 1) * w].copy_from_slice(&buf[y * out.line_size..y * out.line_size + w]);
            }
            Pixels::Indexed { idx, lut }
        } else {
            drop(reader);
            Pixels::Bgra(decode_png_premul(path)?.2)
        };

        let columns = if def.columns == 0 { def.frames } else { def.columns.min(def.frames) };
        let rows = (def.frames + columns - 1) / columns;
        let (fw, fh) = (w / columns, h / rows);
        if fw == 0 || fh == 0 {
            return Err(format!("{}：图片 {w}x{h} 放不下 {} 帧 × {columns} 列", path.display(), def.frames));
        }
        let frame_ms = frame_durations(def, def.frames, None);
        Ok(Sheet { px, stride: w, columns, frames: def.frames, fw, fh, frame_ms, nearest })
    }

    /// GIF 或帧文件夹：各帧竖着拼成一列
    fn load_frames(path: &Path, def: &AnimDef, nearest: bool) -> Result<Sheet, String> {
        let (w, h, strip, natural) = if path.is_dir() { decode_frame_dir(path)? } else { decode_gif(path)? };
        let n = strip.frames;
        if w == 0 || h == 0 || n == 0 {
            return Err(format!("{}：没有可用的帧", path.display()));
        }
        let frame_ms = frame_durations(def, n, natural.as_deref());
        Ok(Sheet { px: strip.finish(), stride: w, columns: 1, frames: n, fw: w, fh: h, frame_ms, nearest })
    }

    /// 第 frame 帧停留多久（毫秒）
    pub fn frame_ms(&self, frame: usize) -> u32 {
        self.frame_ms[frame % self.frames]
    }

    #[inline]
    fn at(&self, i: usize) -> u32 {
        match &self.px {
            Pixels::Indexed { idx, lut } => lut[idx[i] as usize],
            Pixels::Bgra(v) => v[i],
        }
    }

    /// 把第 frame 帧缩放（双线性；像素画用最近邻）画进 dst（dw×dh，预乘 BGRA，自上而下）。
    pub fn draw(&self, frame: usize, dst: &mut [u32], dw: usize, dh: usize) {
        let frame = frame % self.frames;
        let ox = (frame % self.columns) * self.fw;
        let oy = (frame / self.columns) * self.fh;
        let (fw, fh) = (self.fw, self.fh);
        if dw == fw && dh == fh {
            for y in 0..dh {
                let s = (oy + y) * self.stride + ox;
                for x in 0..dw {
                    dst[y * dw + x] = self.at(s + x);
                }
            }
            return;
        }
        if self.nearest {
            let xs: Vec<usize> = (0..dw).map(|x| (x * fw / dw).min(fw - 1)).collect();
            for y in 0..dh {
                let r = (oy + (y * fh / dh).min(fh - 1)) * self.stride + ox;
                let row = &mut dst[y * dw..(y + 1) * dw];
                for (d, &sx) in row.iter_mut().zip(&xs) {
                    *d = self.at(r + sx);
                }
            }
            return;
        }
        let sx = fw as f32 / dw as f32;
        let sy = fh as f32 / dh as f32;
        for y in 0..dh {
            let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (fh - 1) as f32);
            let y0 = fy as usize;
            let y1 = (y0 + 1).min(fh - 1);
            let wy = ((fy - y0 as f32) * 256.0) as u32;
            let r0 = (oy + y0) * self.stride + ox;
            let r1 = (oy + y1) * self.stride + ox;
            for x in 0..dw {
                let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (fw - 1) as f32);
                let x0 = fx as usize;
                let x1 = (x0 + 1).min(fw - 1);
                let wx = ((fx - x0 as f32) * 256.0) as u32;
                let (a, b, c, d) = (self.at(r0 + x0), self.at(r0 + x1), self.at(r1 + x0), self.at(r1 + x1));
                if a == b && b == c && c == d {
                    dst[y * dw + x] = a;
                    continue;
                }
                let mut out = 0u32;
                for sh in [0u32, 8, 16, 24] {
                    let ch = |p: u32| (p >> sh) & 0xff;
                    let top = ch(a) * (256 - wx) + ch(b) * wx;
                    let bot = ch(c) * (256 - wx) + ch(d) * wx;
                    let v = (top * (256 - wy) + bot * wy) >> 16;
                    out |= v.min(255) << sh;
                }
                dst[y * dw + x] = out;
            }
        }
    }

    pub fn bytes(&self) -> usize {
        match &self.px {
            Pixels::Indexed { idx, .. } => idx.len() + 1024,
            Pixels::Bgra(v) => v.len() * 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("claude-pet-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_png(path: &Path, w: u32, h: u32, rgba: [u8; 4]) {
        let mut enc = png::Encoder::new(File::create(path).unwrap(), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let data: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
        enc.write_header().unwrap().write_image_data(&data).unwrap();
    }

    /// frames: (左, 上, 宽, 高, 颜色, 时长厘秒, disposal)
    fn write_gif(path: &Path, w: u16, h: u16, frames: &[(u16, u16, u16, u16, [u8; 4], u16, gif::DisposalMethod)]) {
        let mut enc = gif::Encoder::new(File::create(path).unwrap(), w, h, &[]).unwrap();
        for &(l, t, fw, fh, c, delay, dispose) in frames {
            let mut px: Vec<u8> = (0..fw as usize * fh as usize).flat_map(|_| c).collect();
            let mut f = gif::Frame::from_rgba_speed(fw, fh, &mut px, 10);
            f.left = l;
            f.top = t;
            f.delay = delay;
            f.dispose = dispose;
            enc.write_frame(&f).unwrap();
        }
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    #[test]
    fn natural_order() {
        let mut v = vec!["10.png", "2.png", "1.png", "frame_9.png", "frame_10.png"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, ["1.png", "2.png", "10.png", "frame_9.png", "frame_10.png"]);
    }

    #[test]
    fn state_names() {
        let cases = [
            ("idle", Some("idle")),
            ("IDLE_2", Some("idle")),
            ("click-1", Some("click")),
            ("working", Some("working")),
            ("work 2", Some("working")),
            ("workout", None),
            ("drag_left", Some("drag_left")),
            ("drag", Some("drag")),
            ("dragon", None),
            ("待机2", Some("idle")),
            ("工作中", Some("working")),
            ("等待你确认", Some("waiting")),
            ("挥手", None),
        ];
        for (name, want) in cases {
            assert_eq!(state_of(name), want, "{name}");
        }
    }

    #[test]
    fn gif_disposal_and_delays() {
        use gif::DisposalMethod::*;
        let d = tmp("gif");
        let p = d.join("a.gif");
        // 第 1 帧铺满红色，显示完清成背景；第 2 帧只在右下角画 2x2 蓝色；第 3 帧什么都不清，在左上角画红点
        write_gif(&p, 4, 4, &[(0, 0, 4, 4, RED, 5, Background), (2, 2, 2, 2, BLUE, 0, Keep), (0, 0, 1, 1, RED, 20, Keep)]);
        let def = AnimDef { file: "a.gif".into(), frames: 0, columns: 0, fps: 12.0, durations: vec![] };
        let s = Sheet::load(&p, &def, false).unwrap();
        assert_eq!((s.frames, s.fw, s.fh), (3, 4, 4));
        assert_eq!(s.frame_ms, vec![50, 100, 200]);
        let mut buf = vec![0u32; 16];
        s.draw(1, &mut buf, 4, 4);
        assert_eq!(buf[0] >> 24, 0, "第 1 帧被清成透明");
        assert_eq!(buf[2 * 4 + 2], premul(0, 0, 255, 255));
        s.draw(2, &mut buf, 4, 4);
        assert_eq!(buf[0], premul(255, 0, 0, 255));
        assert_eq!(buf[3 * 4 + 3], premul(0, 0, 255, 255), "蓝色保留");
        assert!(matches!(s.px, Pixels::Indexed { .. }), "颜色少，应该存成调色板");
        // pet.json 里写了 durations 就覆盖 GIF 自带的
        let def = AnimDef { durations: vec![80], ..def };
        assert_eq!(Sheet::load(&p, &def, false).unwrap().frame_ms, vec![80, 80, 80]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn frame_folder_and_auto_pack() {
        let d = tmp("auto");
        let pet = d.join("my-pet");
        std::fs::create_dir_all(pet.join("work")).unwrap();
        // 帧文件夹：10.png 要排在 2.png 后面
        write_png(&pet.join("work").join("1.png"), 3, 2, RED);
        write_png(&pet.join("work").join("10.png"), 3, 2, BLUE);
        write_png(&pet.join("work").join("2.png"), 3, 2, RED);
        let gif1 = |name: &str| write_gif(&pet.join(name), 40, 40, &[(0, 0, 40, 40, RED, 10, gif::DisposalMethod::Keep)]);
        gif1("idle.gif");
        gif1("click-1.gif");
        gif1("思考.gif");
        gif1("wave.gif");
        std::fs::create_dir_all(d.join("empty")).unwrap();

        let pack = Pack::load(&pet).unwrap();
        assert_eq!(pack.name, "my-pet");
        assert_eq!(pack.states["idle"], ["idle"]);
        assert_eq!(pack.states["click"], ["click-1"]);
        assert_eq!(pack.states["thinking"], ["思考"]);
        assert_eq!(pack.states["working"], ["work"]);
        assert_eq!(pack.idle_extras.pool, ["wave"]);
        assert!(pack.pixelated, "40 像素高按像素画处理");
        assert_eq!(pack.scale, 3.0);

        let s = pack.load_sheet("work").unwrap();
        assert_eq!((s.frames, s.fw, s.fh), (3, 3, 2));
        let mut buf = vec![0u32; 6];
        s.draw(2, &mut buf, 3, 2);
        assert_eq!(buf[0], premul(0, 0, 255, 255), "最后一帧是 10.png");

        let names: Vec<String> = list_packs(&d).into_iter().map(|(dir, _)| dir).collect();
        assert_eq!(names, ["my-pet"], "空文件夹不算宠物");

        // 尺寸不一致的帧文件夹要报错
        write_png(&pet.join("work").join("3.png"), 5, 5, RED);
        assert!(pack.load_sheet("work").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bundled_packs_still_load() {
        for (dir, _) in list_packs(Path::new("dist/pets")) {
            let pack = Pack::load(&Path::new("dist/pets").join(&dir)).unwrap();
            for (name, _) in &pack.anims {
                pack.load_sheet(name).unwrap_or_else(|e| panic!("{dir}/{name}: {e}"));
            }
        }
    }
}
