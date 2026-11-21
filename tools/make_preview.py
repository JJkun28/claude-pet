"""生成 README 用的预览动图：三个动画包同步演示 待机→思考→工作→等待→完成。
用法：python -I tools/make_preview.py dist/pets docs/preview.gif（需要 Pillow，字体用 Windows 自带的微软雅黑）
"""
import json
import math
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

PETS_DIR = Path(sys.argv[1])
OUT = Path(sys.argv[2])
PETS = ["ghost-squeak", "zyf", "maid"]
STATES = [("idle", "待机"), ("thinking", "思考中"), ("working", "工作中"), ("waiting", "等待你确认"), ("success", "完成")]
STATE_MS = 2200
TICK = 100  # 每帧 100ms
PET_H = 170
W, H = 720, 300
BG = (250, 247, 242)
INK = (60, 52, 46)
MUTED = (165, 155, 145)
ACCENT = (204, 120, 92)

font = ImageFont.truetype(r"C:\Windows\Fonts\msyh.ttc", 17)
font_b = ImageFont.truetype(r"C:\Windows\Fonts\msyhbd.ttc", 17)


class Anim:
    def __init__(self, pack_dir, spec, pixelated):
        sheet = Image.open(pack_dir / spec["file"]).convert("RGBA")
        n = spec["frames"]
        cols = spec.get("columns", n)
        rows = math.ceil(n / cols)
        fw, fh = sheet.width // cols, sheet.height // rows
        if "durations" in spec:
            self.durs = spec["durations"]
        else:
            self.durs = [1000 / spec.get("fps", 12)] * n
        self.total = sum(self.durs)
        self.pixelated = pixelated
        self.frames = []
        for i in range(n):
            x, y = (i % cols) * fw, (i // cols) * fh
            self.frames.append(sheet.crop((x, y, x + fw, y + fh)))

    def bbox(self):
        boxes = [f.getchannel("A").point(lambda a: 255 if a > 24 else 0).getbbox() for f in self.frames]
        boxes = [b for b in boxes if b]
        return min(b[0] for b in boxes), min(b[1] for b in boxes), max(b[2] for b in boxes), max(b[3] for b in boxes)

    def fit(self, box, scale):
        resample = Image.NEAREST if self.pixelated else Image.LANCZOS
        w, h = box[2] - box[0], box[3] - box[1]
        size = (max(1, round(w * scale)), max(1, round(h * scale)))
        self.frames = [f.crop(box).resize(size, resample) for f in self.frames]

    def at(self, ms):
        t = ms % self.total
        for f, d in zip(self.frames, self.durs):
            if t < d:
                return f
            t -= d
        return self.frames[-1]


packs = []
for name in PETS:
    d = PETS_DIR / name
    meta = json.loads((d / "pet.json").read_text(encoding="utf-8"))
    anims = {}
    for st, _ in STATES:
        pool = meta["states"].get(st) or meta["states"]["idle"]
        anims[st] = Anim(d, meta["animations"][pool[0]], meta.get("pixelated", False))
    # 所有动作共用一个内容边界：去掉精灵图里的空白，再统一缩放到 PET_H 高
    bs = [a.bbox() for a in anims.values()]
    box = (min(b[0] for b in bs), min(b[1] for b in bs), max(b[2] for b in bs), max(b[3] for b in bs))
    scale = min(PET_H / (box[3] - box[1]), (W / len(PETS) - 16) / (box[2] - box[0]))
    for a in anims.values():
        a.fit(box, scale)
    packs.append(anims)


def pill_row(draw, active):
    labels = [lab for _, lab in STATES]
    widths = [draw.textlength(l, font=font_b) + 28 for l in labels]
    gap = 10
    x = (W - sum(widths) - gap * (len(labels) - 1)) / 2
    y = 22
    for i, (lab, w) in enumerate(zip(labels, widths)):
        on = i == active
        if on:
            draw.rounded_rectangle((x, y, x + w, y + 34), radius=17, fill=ACCENT)
        else:
            draw.rounded_rectangle((x, y, x + w, y + 34), radius=17, outline=(225, 216, 206), width=2)
        draw.text((x + w / 2, y + 17), lab, font=font_b if on else font, fill=(255, 255, 255) if on else MUTED, anchor="mm")
        x += w + gap


frames = []
total_ms = STATE_MS * len(STATES)
for t in range(0, total_ms, TICK):
    si = t // STATE_MS
    st = STATES[si][0]
    local = t - si * STATE_MS
    img = Image.new("RGBA", (W, H), BG + (255,))
    draw = ImageDraw.Draw(img)
    pill_row(draw, si)
    slot = W / len(packs)
    for i, anims in enumerate(packs):
        f = anims[st].at(local)
        x = int(slot * i + (slot - f.width) / 2)
        img.alpha_composite(f, (x, H - 14 - f.height))
    frames.append(img.convert("RGB").quantize(colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE))

OUT.parent.mkdir(parents=True, exist_ok=True)
frames[0].save(OUT, save_all=True, append_images=frames[1:], duration=TICK, loop=0, optimize=True, disposal=1)
print(OUT, f"{OUT.stat().st_size / 1e6:.2f} MB", len(frames), "frames")
