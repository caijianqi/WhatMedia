#!/usr/bin/env python3
"""往开发库里灌一批**演示数据**，好让界面在没有真媒体文件时也能看。

为什么需要它：新装的库是空的，而空库的首页只显示"开始使用"引导 ——
Hero 轮播、海报墙、分类 tab、详情页演职员与季列表全都看不到。
要评审界面就得先有内容，而"先去扫一个真实目录"这件事在只有几 GB
测试素材的机器上并不现实。

刻意**不造假文件**：`media_file` / `media_version` 一行都不插。
所以这些条目能浏览、能看详情，但点播放会报"没有可播放的文件" ——
这是对的，界面评审不需要能播，而假造一条指向不存在文件的版本会让
后面排查真实播放问题的人被误导。

用法：
    python scripts/seed_demo.py                      # 追加演示数据
    python scripts/seed_demo.py --wipe               # 先清空再灌（可重复跑）
    python scripts/seed_demo.py --db path/to/x.db
"""

import argparse
import json
import sqlite3
import sys
import uuid
from datetime import datetime, timedelta, timezone

# 12 部电影。评分特意拉开档次：Hero 取"评分最高的 5 部"，
# 有梯度才看得出它取的是高分而不是最新。
MOVIES = [
    ("沙丘", 2021, 8.0, 9_300_000, "科幻", "厄拉科斯星球出产宇宙中最珍贵的香料，谁控制它谁就控制了整个帝国。"),
    ("银翼杀手 2049", 2017, 8.3, 9_840_000, "科幻", "一名新的银翼杀手揭开了一个埋藏三十年的秘密。"),
    ("降临", 2016, 7.8, 8_460_000, "科幻", "十二艘外星飞船降临地球，语言学家被派去破译它们的文字。"),
    ("疯狂的麦克斯4", 2015, 8.6, 7_200_000, "动作", "在荒漠世界里，一场横穿荒野的逃亡持续了整整两天。"),
    ("谍影重重", 2002, 7.9, 7_140_000, "动作", "一个失忆的男人从海里被捞起，身上只有一个瑞士银行的账号。"),
    ("寄生虫", 2019, 8.7, 7_920_000, "剧情", "一家人一步步渗入豪门，直到地下室的秘密被揭开。"),
    ("花样年华", 2000, 8.2, 5_880_000, "剧情", "两个相邻而居的人，在狭窄的走廊里反复错过。"),
    ("千与千寻", 2001, 9.3, 7_500_000, "动画", "女孩误入神灵的世界，为赎回父母而在汤屋里工作。"),
    ("蜘蛛侠：平行宇宙", 2018, 8.5, 7_020_000, "动画", "来自不同宇宙的蜘蛛侠聚到了同一个城市。"),
    ("消失的爱人", 2014, 8.1, 8_940_000, "悬疑", "妻子在结婚五周年当天失踪，丈夫成了头号嫌疑人。"),
    ("布达佩斯大饭店", 2014, 8.1, 5_940_000, "喜剧", "一位礼宾司与他门生的传奇经历，以及一幅名画的归属。"),
    ("触不可及", 2011, 8.4, 6_720_000, "剧情", "一个贵族和他的看护，两个世界的人彼此改变了对方。"),
]

CAST = [
    ("提莫西·查拉梅", "保罗"),
    ("丽贝卡·弗格森", "杰西卡"),
    ("奥斯卡·伊萨克", "雷托公爵"),
    ("赞达亚", "契妮"),
    ("乔什·布洛林", "古尼"),
]

CREW = [("丹尼斯·维伦纽瓦", "director"), ("乔·沃克", "editor"), ("汉斯·季默", "composer")]

EPISODE_TITLES = [
    "第一集", "第二集", "第三集", "第四集", "第五集", "第六集",
]


def person_id(cur: sqlite3.Cursor, name: str) -> str:
    """按名字取 person，没有才建。

    `person` 表没有唯一约束，`INSERT OR IGNORE` 挡不住重名 ——
    同一个"汉斯·季默"会被插成好几行，演职员表于是出现同名的多个人。
    所以先查再插，把去重放在这里而不是指望数据库。
    """
    row = cur.execute("SELECT id FROM person WHERE name = ?", (name,)).fetchone()
    if row:
        return row[0]
    pid = str(uuid.uuid4())
    cur.execute("INSERT INTO person (id, name) VALUES (?,?)", (pid, name))
    return pid


def genre_id(cur: sqlite3.Cursor, name: str) -> str:
    """按名字取 genre，没有才建。

    `genre.id` 是显式主键、没有默认值 —— 只插 name 会得到一个 id 为 NULL 的行，
    而 `item_genre` 引用的是那个 NULL，于是类型永远关联不上。
    """
    row = cur.execute("SELECT id FROM genre WHERE name = ?", (name,)).fetchone()
    if row:
        return row[0]
    gid = str(uuid.uuid4())
    cur.execute("INSERT INTO genre (id, name) VALUES (?,?)", (gid, name))
    return gid


def now(offset_days: int = 0) -> str:
    t = datetime.now(timezone.utc) - timedelta(days=offset_days)
    return t.isoformat().replace("+00:00", "Z")


def seed(conn: sqlite3.Connection) -> None:
    cur = conn.cursor()

    lib_id = str(uuid.uuid4())
    cur.execute(
        "INSERT INTO library (id, name, kind, storage_provider, root_path, scan_policy,"
        " metadata_providers, created_at) VALUES (?,?,?,?,?,?,?,?)",
        (
            lib_id,
            "演示库",
            "movie",
            "local",
            "D:/Media",
            "balanced",
            json.dumps(["tmdb"], ensure_ascii=False),
            now(),
        ),
    )

    # ── 电影 ──────────────────────────────────────────────
    movie_ids = []
    for i, (title, year, rating, runtime, genre, overview) in enumerate(MOVIES):
        item_id = str(uuid.uuid4())
        movie_ids.append(item_id)
        cur.execute(
            "INSERT INTO media_item (id, library_id, kind, title, sort_title, year, overview,"
            " runtime_ms, rating, locked_fields, added_at, match_title)"
            " VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                item_id,
                lib_id,
                "movie",
                f"演示·{title}",
                title,
                year,
                overview,
                runtime,
                rating,
                "[]",
                now(len(MOVIES) - i),
                title,
            ),
        )
        cur.execute(
            "INSERT OR IGNORE INTO item_genre (item_id, genre_id) VALUES (?,?)",
            (item_id, genre_id(cur, genre)),
        )

    # 演职员只给前 5 部：一屏里演职员表都一样反而看不出是"每部各自的"。
    for item_id in movie_ids[:5]:
        for sort, (name, character) in enumerate(CAST):
            cur.execute(
                "INSERT OR IGNORE INTO credit (person_id, item_id, role, character, sort_order)"
                " VALUES (?,?,'actor',?,?)",
                (person_id(cur, name), item_id, character, sort),
            )
        for sort, (name, role) in enumerate(CREW):
            cur.execute(
                "INSERT OR IGNORE INTO credit (person_id, item_id, role, character, sort_order)"
                " VALUES (?,?,?,NULL,?)",
                (person_id(cur, name), item_id, role, sort),
            )

    # ── 剧集：2 季 + 11 集 ─────────────────────────────────
    series_id = str(uuid.uuid4())
    cur.execute(
        "INSERT INTO series (id, title, year, overview, locked_fields, match_title)"
        " VALUES (?,?,?,?,?,?)",
        (
            series_id,
            "演示剧集",
            2021,
            "一栋公寓里住了六户人家，十年间他们各自老去。",
            "[]",
            "演示剧集",
        ),
    )

    for season_no, count in ((1, 6), (2, 5)):
        season_id = str(uuid.uuid4())
        cur.execute(
            "INSERT INTO season (id, series_id, season_number, title) VALUES (?,?,?,?)",
            (season_id, series_id, season_no, None),
        )
        for ep_no in range(1, count + 1):
            item_id = str(uuid.uuid4())
            cur.execute(
                "INSERT INTO media_item (id, library_id, kind, title, sort_title, year, overview,"
                " runtime_ms, rating, locked_fields, added_at, match_title)"
                " VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                (
                    item_id,
                    lib_id,
                    "episode",
                    f"演示剧集 S{season_no}E{ep_no} {EPISODE_TITLES[(ep_no - 1) % len(EPISODE_TITLES)]}",
                    f"S{season_no}E{ep_no}",
                    2021,
                    "本集简介。",
                    2_700_000,
                    7.6,
                    "[]",
                    now(30 - ep_no),
                    f"S{season_no}E{ep_no}",
                ),
            )
            cur.execute(
                "INSERT INTO episode (id, season_id, episode_number, item_id, title)"
                " VALUES (?,?,?,?,?)",
                (
                    str(uuid.uuid4()),
                    season_id,
                    ep_no,
                    item_id,
                    EPISODE_TITLES[(ep_no - 1) % len(EPISODE_TITLES)],
                ),
            )

    # ── 续播点 ────────────────────────────────────────────
    # 续播要满足 stopped_at IS NULL 且进度在 5%~95% 之间，
    # 所以 position 取时长的 40% 左右。
    user = cur.execute("SELECT id FROM user LIMIT 1").fetchone()
    if user:
        for i, item_id in enumerate(movie_ids[:3]):
            cur.execute(
                "INSERT INTO playback_session (id, user_id, item_id, file_id, strategy,"
                " position_ms, duration_ms, updated_at, client_id, created_at, stopped_at)"
                " VALUES (?,?,?,?,'direct',?,?,?,?,?,NULL)",
                (
                    str(uuid.uuid4()),
                    user[0],
                    item_id,
                    str(uuid.uuid4()),
                    int(MOVIES[i][3] * 0.42),
                    MOVIES[i][3],
                    now(i),
                    "seed-demo",
                    now(i),
                ),
            )


def wipe(conn: sqlite3.Connection) -> None:
    cur = conn.cursor()
    for t in (
        "credit", "item_genre", "genre", "person", "episode", "season",
        "series", "playback_session", "media_item", "library",
    ):
        cur.execute(f"DELETE FROM {t}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--db", default=".dev-data/smoke.db", help="SQLite 库路径")
    ap.add_argument("--wipe", action="store_true", help="先清掉已有的媒体数据再灌")
    args = ap.parse_args()

    conn = sqlite3.connect(args.db)
    try:
        if args.wipe:
            wipe(conn)
            conn.commit()
        seed(conn)
        conn.commit()
    finally:
        conn.close()

    print(f"演示数据已写入 {args.db}")
    print("注意：只写了浏览用的元数据，没有 media_file —— 这些条目能看不能播。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
