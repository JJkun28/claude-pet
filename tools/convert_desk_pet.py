"""把 desk-pet（tg111/claude-code-desk-pet）的预览 GIF 转成 claude-pet 的动画包。

用法（在任意目录运行，参数都是路径）：
    python -I tools/convert_desk_pet.py <desk-pet 仓库里的 desk-pet/assets 目录> <输出目录，如 dist/pets/maid>

做的事：
  1. 读 assets/config.jsonc：待机 / 拖拽 / 点击 / 随机动作分类 / 工作状态 各用哪些动画
  2. 读 scripts/encode_preview_gifs.py 里的 PINYIN 表：动画中文名 → 预览 GIF 文件名
  3. 每段 GIF → 一张调色板 PNG 精灵图（每像素 1 字节，透明色单独一格），写 pet.json

素材版权归 desk-pet 原作者（https://github.com/PC2005-cloud/dsh-pet），仅限个人非商用，
分发时必须保留原作者地址。
"""

import ast
import json
import math
import sys
from pathlib import Path

from PIL import Image, ImageSequence

COLUMNS = 12


def strip_jsonc(text: str) -> str:
    """去掉 // 与 /* */ 注释和尾逗号，字符串内的内容原样保留。"""
    out, i, n = [], 0, len(text)
    in_str = False
    while i < n:
        c = text[i]
        if in_str:
            out.append(c)
            if c == "\\" and i + 1 < n:
                out.append(text[i + 1])
                i += 2
                continue
            if c == '"':
                in_str = False
            i += 1
            continue
        if c == '"':
            in_str = True
            out.append(c)
            i += 1
        elif text.startswith("//", i):
            while i < n and text[i] != "\n":
                i += 1
        elif text.startswith("/*", i):
            end = text.find("*/", i + 2)
            i = n if end < 0 else end + 2
        else:
            out.append(c)
            i += 1
    s = "".join(out)
    # 尾逗号
    import re

    return re.sub(r",(\s*[}\]])", r"\1", s)


def load_pinyin(script: Path) -> dict:
    src = script.read_text(encoding="utf-8")
    start = src.index("PINYIN = {")
    depth, j = 0, src.index("{", start)
    for k in range(j, len(src)):
        if src[k] == "{":
            depth += 1
        elif src[k] == "}":
            depth -= 1
            if depth == 0:
                return ast.literal_eval(src[j : k + 1])
    raise ValueError("PINYIN 表没找到结尾")


def gif_to_sheet(gif: Path, out_png: Path):
    im = Image.open(gif)
    duration = im.info.get("duration") or 80
    frames = [f.convert("RGBA") for f in ImageSequence.Iterator(im)]
    w, h = frames[0].size
    cols = min(COLUMNS, len(frames))
    rows = math.ceil(len(frames) / cols)
    sheet = Image.new("RGBA", (w * cols, h * rows), (0, 0, 0, 0))
    for idx, f in enumerate(frames):
        # GIF 只有 1 位透明：半透明一律当不透明，避免量化出脏边
        px = f.getchannel("A").point(lambda a: 255 if a >= 128 else 0)
        f.putalpha(px)
        sheet.paste(f, ((idx % cols) * w, (idx // cols) * h))
    q = sheet.quantize(colors=255, method=Image.Quantize.FASTOCTREE)
    q.save(out_png, optimize=True)
    return {"frames": len(frames), "columns": cols, "fps": round(1000 / duration, 2)}, (w, h)


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    assets = Path(sys.argv[1]).resolve()
    out = Path(sys.argv[2]).resolve()
    repo_desk_pet = assets.parent
    cfg = json.loads(strip_jsonc((assets / "config.jsonc").read_text(encoding="utf-8")))
    anims_cfg = cfg["animations"]
    pinyin = load_pinyin(repo_desk_pet.parent / "scripts" / "encode_preview_gifs.py")

    work = anims_cfg["events"]["workStatus"]  # 0 thinking 1 working 2 result 3 waiting 4 success 5 error
    as_list = lambda v: v if isinstance(v, list) else [v]
    states = {
        "idle": as_list(anims_cfg["idle"]),
        "thinking": as_list(work[0]),
        "working": as_list(work[1]),
        "result": as_list(work[2]),
        "waiting": as_list(work[3]),
        "success": as_list(work[4]),
        "error": as_list(work[5]),
        "drag": as_list(anims_cfg["drag"]),
        "click": as_list(anims_cfg["clicks"]),
    }

    menu = [
        {"group": "工作状态", "items": [n for k in ("thinking", "working", "result", "waiting", "success", "error") for n in states[k]]},
        {"group": "待机与互动", "items": states["idle"] + as_list(anims_cfg.get("turn", [])) + states["drag"] + states["click"]},
        {"group": "移动", "items": [m["name"] for m in anims_cfg["moves"]["actions"]]},
    ]
    idle_pool = []
    for cat in anims_cfg["categories"]:
        menu.append({"group": cat["id"], "items": list(cat["actions"])})
        if not cat.get("noMirror"):
            idle_pool += cat["actions"]
    menu.append({"group": "碎碎念", "items": as_list(anims_cfg["events"]["whisper"])})

    wanted = []
    for g in menu:
        for n in g["items"]:
            if n not in wanted:
                wanted.append(n)

    (out / "anims").mkdir(parents=True, exist_ok=True)
    animations, missing, size = {}, [], None
    for name in wanted:
        py = pinyin.get(name)
        gif = assets / "preview" / f"{py}.gif" if py else None
        if not gif or not gif.exists():
            missing.append(name)
            continue
        info, wh = gif_to_sheet(gif, out / "anims" / f"{py}.png")
        size = size or wh
        animations[name] = {"file": f"anims/{py}.png", **info}
        print(f"  {name} -> {py}.png ({info['frames']} 帧)")

    have = lambda xs: [x for x in xs if x in animations]
    pet = {
        "name": "蓝发小女仆",
        "credit": "素材来自 desk-pet / dsh-pet，原作者 https://github.com/PC2005-cloud/dsh-pet ，仅限个人非商用，分发须保留原作者地址",
        "scale": 1.5,
        "animations": animations,
        "states": {k: have(v) for k, v in states.items()},
        "idle_extras": {"min_secs": 30, "max_secs": 90, "pool": have(idle_pool)},
        "menu": [{"group": g["group"], "items": have(g["items"])} for g in menu if have(g["items"])],
    }
    (out / "pet.json").write_text(json.dumps(pet, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"完成：{len(animations)} 段动画，帧尺寸 {size}，缺失 {len(missing)}：{missing}")


if __name__ == "__main__":
    main()
