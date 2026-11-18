//! 动画包：pets/<名字>/pet.json + 每段动画一张精灵图（PNG，按网格排帧）。
//!
//! 内存策略：只解码正在播的几段动画（App 里有个小 LRU）。调色板 PNG 原样按「每像素 1 字节」
//! 存，渲染时查表；普通 RGBA PNG 每像素 4 字节。缩放在每帧渲染时做，不预先放大。

use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Clone)]
pub struct AnimDef {
    pub file: String,
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
        let Ok(text) = std::fs::read_to_string(e.path().join("pet.json")) else { continue };
        let dir = e.file_name().to_string_lossy().into_owned();
        let name = serde_json::from_str::<Head>(text.trim_start_matches('\u{feff}'))
            .ok()
            .map(|h| h.name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| dir.clone());
        out.push((dir, name));
    }
    out.sort();
    out
}

impl Pack {
    pub fn load(dir: &Path) -> Result<Pack, String> {
        let path = dir.join("pet.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("读不到 {}：{e}", path.display()))?;
        let pf: PackFile = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("pet.json 格式错误：{e}"))?;
        let mut anims = Vec::new();
        for (name, v) in pf.animations {
            let def: AnimDef = serde_json::from_value(v).map_err(|e| format!("动画「{name}」配置错误：{e}"))?;
            if def.frames == 0 {
                return Err(format!("动画「{name}」frames 不能为 0"));
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
            name: pf.name,
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

enum Pixels {
    /// 每像素 1 字节调色板索引 + 预乘好的 BGRA 查找表
    Indexed { idx: Vec<u8>, lut: Box<[u32; 256]> },
    /// 每像素一个预乘 BGRA
    Bgra(Vec<u32>),
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
            let mut dec = png::Decoder::new(open()?);
            dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
            let mut reader = dec.read_info().map_err(err)?;
            let mut buf = vec![0u8; reader.output_buffer_size()];
            let out = reader.next_frame(&mut buf).map_err(err)?;
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
            Pixels::Bgra(v)
        };

        let columns = if def.columns == 0 { def.frames } else { def.columns.min(def.frames) };
        let rows = (def.frames + columns - 1) / columns;
        let (fw, fh) = (w / columns, h / rows);
        if fw == 0 || fh == 0 {
            return Err(format!("{}：图片 {w}x{h} 放不下 {} 帧 × {columns} 列", path.display(), def.frames));
        }
        let fps_ms = (1000.0 / def.fps.clamp(1.0, 60.0)).round() as u32;
        let frame_ms = (0..def.frames)
            .map(|i| def.durations.get(i).or(def.durations.last()).copied().unwrap_or(fps_ms).clamp(16, 10_000))
            .collect();
        Ok(Sheet { px, stride: w, columns, frames: def.frames, fw, fh, frame_ms, nearest })
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
