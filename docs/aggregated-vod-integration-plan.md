# 本地媒体库 × 全网多源聚合：整合方案

> 目标：**把「全网聚合」从顶栏搜索框的一个下拉分区，升级为与本地库对等的「云端库」**，
> 让它走完发现 → 搜索 → 详情 → 播放 → 回流 的完整生命周期。
>
> 本文只做方案设计，不含代码改动。所有涉及的行号基于当前工作区快照。

---

## 一、结论先行

后端聚合能力已经齐备，**短板 100% 在前端暴露面**。判断依据：

| 环节 | 后端能力 | 前端暴露 |
| :--- | :--- | :--- |
| 并发调度 | `AggregatorEngine::search` JoinSet 广播（`engine.rs:67`） | — |
| 容错 | 滑动窗口熔断器（`circuit_breaker.rs`） | 无可见状态 |
| 聚类 | `clean_title + year` 精确 + Levenshtein 模糊（`normalizer.rs:101`） | 仅在下拉里体现为「N 线路」 |
| 流式 | SSE `local` / `remote_source` / `remote_cluster` 三事件（`aggregator.rs:155`） | **从未被消费** |
| 缓存 | `vod_search_cache` 表 + `get/set_search_cache` 已实现（`repo/aggregator.rs:85`） | **从未被调用** |
| 反代 | M3U8 重写绕过防盗链（`aggregator.rs:369`） | 已用于播放 |
| 门禁 | 儿童账号屏蔽远端（`aggregator.rs:121`） | 已生效 |
| AI | `search_aggregated_vod` 工具（`tools.rs:613`） | AssistView 可用 |

一句话：**地基打好了，房子没盖。** 本文就是房子的施工图。

---

## 二、现状断点清单（按严重度排序）

1. **搜索结果无处可去**（最严重）
   `TopBar.vue:110` 的 `submit()` 只做 `router.push({ name: 'library' })`，**不带 `q`**。
   `LibraryView.vue` 只读 `kind/sort/order/page`，没有关键词框 → 回车等于丢弃搜索。
   侧边栏「搜索」项（`AppSidebar.vue:118`）跳 `/library?focus=search`，`focus` 是个**无人读取的死参数**。

2. **同片重影**
   下拉浮层把结果切成「本地媒体库」与「云端聚合在线源」两个 section（`TopBar.vue:238/254`）。
   同一部片会各出现一次，用户得自己判断是不是同一部。

3. **在线播放无进度**
   `stores/player.ts:338 openVodUrl()` 不创建 playback session → 无续播、不进首页「继续观看」、关掉即失忆。

4. **播放后不能换源**
   换源面板只存在于播放**前**的 `VodDetailModal`。一旦进入 `/play/vod`，线路信息丢失，卡住只能退出重搜。

5. **云端详情不可寻址**
   `VodDetailModal` 是纯弹窗，无 URL：不能分享、不能前进后退、刷新即丢。

6. **收藏不支持在线条目**
   `favorite.item_id` 外键引用 `media_item(id)`（`0007_social.sql`），在线源无 `item_id`，物理上塞不进。

7. **首页无全网发现位**
   HomeView 只有 Hero / 继续观看 / 最近添加 / 为你发现 / 按类型浏览，全部基于本地库。
   用户不主动搜索就永远看不到在线源存在。

8. **TV 端搜索框被隐藏**
   `App.vue:89` 传 `:show-search="!layout.isTv"` → Android TV 上聚合搜索**完全不可达**。

---

## 三、MoonTV 对标

### 3.1 它怎么用多源聚合

MoonTV 是 Next.js 14 的**纯在线**聚合播放器（无本地库概念，`config.json` 挂 20+ 苹果 CMS V10 源）。闭环：

- **首页**：继续观看 + 豆瓣三行（热门电影 / 热门剧集 / 热门综艺，走 `custom_category`），`Promise.all` 并发，`ScrollableRow` 横滑，各带「更多」。
- **搜索页**：`agg` 聚合视图 ⇄ `all` 全源视图切换；聚合键 `title + year + type`，海报/集数取**众数**；完整搜索历史（chip 悬停删除）；`SEARCH_MAX_PAGE` 默认 5；黄色内容关键词过滤。
- **播放页**：ArtPlayer + HLS.js；「选集 / 换源」双 Tab；`preferBestSource()` 按**画质 40% + 速度 40% + 延迟 20%** 给线路评分优选（4s 超时，结果复用）；跳片头片尾；每 5 秒写 PlayRecord。
- **工程**：`cache_time` 默认 7200s + 三级 CDN 缓存头；单源失败内部吞掉；`/admin` 拖拽排序改源。

### 3.2 差异的本质

| | MoonTV | WhatMedia |
| :--- | :--- | :--- |
| 在线源的地位 | 一等公民，贯穿五段链路 | 搜索框附属品，只占一段 |
| 本地库 | 无 | 有一等公民，链路完整 |
| 内容发现 | 豆瓣榜单（外部榜单驱动） | 本地 Hero + 分类（库内数据驱动） |
| 数据持久化 | 收藏/进度四种后端 | 收藏/进度绑定 `media_item` 外键 |

**抄什么**：播放页的换源优选评分、三级缓存思路、搜索历史、聚合/全源双视图。
**不抄什么**：豆瓣榜单做首页——WhatMedia 已有 TMDB/Bangumi provider 和本地评分，
应该做**本地库驱动 + 在线源补全**的发现，而不是引入第三方榜单当门面。

### 3.3 WhatMedia 能做而 MoonTV 做不到的（护城河）

1. **同片质量抉择**：本地有 4K 原盘时明确提示「不必看在线 1080p」——MoonTV 没有本地概念，给不出这个判断。
2. **想看清单双向到货**：库里没有的片加入想看 → 在线源上线了 / 本地入库了，双向通知。
3. **家庭分级**：`CHILD_RESTRICTED` 已在后端做死，比 MoonTV 的关键词黄滤严格一个量级。

---

## 四、目标形态

### 4.1 信息架构

```
首页 /
├── Hero 轮播                        [本地]
├── 继续观看                          [本地 + 在线] ← 改造
├── 最近添加                          [本地]
├── 全网热播                          [在线] ← 新增
├── 想看清单                          [在线+本地] ← 新增
└── 按类型浏览                        [本地]

搜索结果页 /search?q=                ← 新增路由
├── Tab: 全部 / 本地 / 全网
├── 同片合并卡（本地原画 + N 条在线线路）
└── 搜索历史 chips

云端详情页 /vod/:key                  ← 新增路由
├── 海报 / 年份 / 演员 / 简介
├── 线路列表（带评分）
└── 选集 + 收藏

播放页 /play/vod?key=&route=&ep=
└── 侧栏：选集 / 换源                ← 新增
```

### 4.2 用户旅程（改造后）

```
搜「奥本海默」
  → /search?q= 出一张合并卡：「本地 4K 原盘」+「在线 8 条线路」
  → 点在线 → /vod/:key 云端详情页（可分享）
  → 选线路 → /play/vod 播放，侧栏随时换源
  → 看一半退出 → 首页「继续观看」出现该在线条目
  → 库里没有的片 → 加入想看 → 有源时通知
```

---

## 五、分阶段实施

### P0 · 补断裂（最高性价比，不动后端契约即可完成大半）

**目标**：让搜索结果有地方去、消除重影、在线播放有进度。

#### P0-1 新增搜索结果页 `/search?q=`

- 路由：`client/src/router/index.ts` 加 `{ path: '/search', name: 'search', component: SearchView }`。
- `TopBar.vue` 的 `submit()` 改为 `router.push({ name: 'search', query: { q: query.value } })`；
  下拉底部加「查看「xxx」的全部结果」条目。
- `AppSidebar.vue:118` 的「搜索」项改为跳 `{ name: 'search' }`，顺手删掉死参数 `focus`。
- `App.vue:89` 的 TV 端：不显示输入框，但保留一个「搜索」导航入口指向 `/search`，否则 TV 端仍不可达。
- 新组件 `client/src/views/SearchView.vue`：三 Tab（全部 / 本地 / 全网），
  复用 `MediaRow` / `PosterCard` 的海报网格，云端卡片复用 `VodDetailModal` 的线路渲染逻辑。

#### P0-2 同片合并（消除重影）

**放后端做**，因为聚类所需的 `TitleNormalizer` 在 Rust，前端没有等价实现。

- 后端 `AggregateSearchResponse` 增字段：

  ```rust
  pub struct AggregateSearchResponse {
      pub local: Page<ItemSummary>,
      pub remote: Vec<AggregatedVodItem>,
      /// 同片合并后的统一视图（新增，前端优先消费这个）
      pub merged: Vec<MergedHit>,
  }

  pub struct MergedHit {
      pub match_key: String,          // clean_title + year
      pub title: String,
      pub year: Option<u32>,
      pub poster: Option<String>,
      pub local: Option<ItemSummary>,        // 本地命中（有则优先展示）
      pub remote: Option<AggregatedVodItem>, // 在线聚合卡
  }
  ```

- 合并算法：对本地 `ItemSummary.title` 跑同一个 `TitleNormalizer::clean()`，
  与 remote 的 `clean_title` 比对；`year` 容差 ±1（本地元数据年份常与采集站不一致）。
  两侧都命中 → 一张卡；只有一侧 → 单来源卡。
- 前端卡片 badge：`本地原画` / `在线 N 线路` / `原画 + 在线 N 线路`。
- **保留 `local` / `remote` 原始字段不删**，旧调用方（AI 工具、CLI）不受影响。

#### P0-3 在线播放计入续播

**硬约束**：`playback_session.item_id` / `file_id` 是 `NOT NULL` 且外键引用
`media_item` / `media_file`（`0001_init.sql:157`），在线源塞不进去。
**决策：不改这两个外键，新增独立表**（理由见 §7.1）。

- 迁移 `0010_vod_progress.sql`：

  ```sql
  CREATE TABLE IF NOT EXISTS vod_progress (
      user_id       TEXT NOT NULL,
      vod_key       TEXT NOT NULL,   -- 聚合实体 key（见 P1-1）
      title         TEXT NOT NULL,
      poster        TEXT,
      source_id     TEXT NOT NULL,
      route_index   INTEGER NOT NULL DEFAULT 0,
      episode_index INTEGER NOT NULL DEFAULT 0,
      episode_name  TEXT,
      position_ms   INTEGER NOT NULL DEFAULT 0,
      duration_ms   INTEGER,
      updated_at    TEXT NOT NULL,
      PRIMARY KEY (user_id, vod_key)
  );
  CREATE INDEX IF NOT EXISTS idx_vod_progress_user ON vod_progress(user_id, updated_at DESC);
  ```

- 前端 `stores/player.ts` 的 `openVodUrl()` 增加进度上报：每 10 秒 + 暂停/卸载时
  `PUT /api/v1/vod/progress`。
- 首页 `HomeView` 的「继续观看」行合并本地 `resume` 与在线 `vod_progress`，
  在线卡用 `ContinueCard` 的变体（加「在线」角标）。

**验收**：搜一部在线片 → 播 30 秒 → 退出 → 首页「继续观看」出现该片且进度 ≈ 30 秒。

---

### P1 · 生命周期（让在线源有身份）

#### P1-1 云端实体可寻址

问题：聚合 id 是运行时派生的（SHA256 of clean_title），不持久，下次搜索可能变。

- 迁移同 `0010`：

  ```sql
  CREATE TABLE IF NOT EXISTS vod_entity (
      vod_key     TEXT PRIMARY KEY,  -- 稳定 key: clean_title + year 的哈希
      payload     TEXT NOT NULL,     -- AggregatedVodItem 序列化 JSON
      created_at  TEXT NOT NULL,
      expires_at  INTEGER NOT NULL   -- 建议 6 小时，播放链接会失效
  );
  ```

- 搜索聚合时顺带 `upsert` 到 `vod_entity`；新增 `GET /api/v1/vod/{key}` 返回详情。
- 前端新增 `client/src/views/VodDetailView.vue`（路由 `/vod/:key`），
  内容从 `VodDetailModal.vue` 平移（海报/年份/导演/主演/简介/线路/选集），
  弹窗降级为移动端快捷预览，桌面端点「查看详情」进路由页。

#### P1-2 收藏支持在线条目

`favorite.item_id` 外键同样解不开，新增表（同 §7.1 决策）：

```sql
CREATE TABLE IF NOT EXISTS vod_favorite (
    user_id    TEXT NOT NULL,
    vod_key    TEXT NOT NULL,
    title      TEXT NOT NULL,
    poster     TEXT,
    year       INTEGER,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, vod_key)
);
```

- API：`POST /api/v1/vod/favorite`、`GET /api/v1/vod/favorites`、`DELETE /api/v1/vod/favorite/{key}`。
- 前端 `FavoritesView.vue` 增加「在线收藏」分区；`VodDetailView` 加收藏按钮。

#### P1-3 本地详情页挂「在线补片」

`DetailView.vue` 的 actions 区（约 314 行）在「加入收藏」后加一个按钮
「在线线路 · N」（N 由 `GET /api/v1/search/aggregate?q={title}&merge=true` 惰性查询得出，
懒加载、失败则隐藏）。

点击进入 → 直接打开该片的 `VodDetailView`，让用户选线路。
**语义是「补片」而非「替代」**：本地有片但版本残缺（枪版、缺集）时最有用。

**验收**：本地详情页出现「在线线路」按钮 → 点击 → 进入 `/vod/:key` → 可播放；刷新页面仍在。

---

### P2 · 播放体验（抄 MoonTV 的强项）

#### P2-1 播放页换源 + 选集侧栏

- `PlayerView.vue` 的 `isVod` 分支（第 30 行起）目前只从 `route.query` 读单个 `vod_url`。
  改为读 `vod_key`，加载时调 `GET /api/v1/vod/{key}` 拿到全部 routes。
- 侧栏复用 `EpisodeSelector` 式双 Tab：**选集**（当前线路集数，当前集高亮）+ **换源**（线路列表）。
- 换源不重载页面：直接切 `openVodUrl(newUrl, { startMs: 当前进度 })`。

#### P2-2 线路评分优选（对齐 MoonTV `preferBestSource`）

- 后端新增 `GET /api/v1/vod/{key}/probe`：并发探测各线路首条 m3u8，
  取分辨率（解析 `#EXT-X-STREAM-INF` 或首分片）、响应延迟、下载速度。
- 评分：`quality 40% + speed 40% + latency 20%`，单源超时 4s，失败记 0 不影响其他源。
- 结果缓存进 `vod_entity` 的 `payload`（增 `route_scores` 字段），避免每次播放重测。
- 前端换源列表按分数降序，显示 `1080p · 快 · 120ms` 这类徽章。
- 设置项 `enable_vod_route_optimization` 可关（对齐 MoonTV 的 `enableOptimization`）。

**验收**：一条线路故意填死链 → 探测失败但其余线路正常出分 → 播放页换源列表按分排序。

---

### P3 · 发现位（WhatMedia 的差异化）

#### P3-1 全网热播行

- 后端：`GET /api/v1/vod/discover?kind=movie|tv`，从启用源拉取最新/热门
  （MacCMS 支持 `ac=videolist` 与排序参数），结果走同一套聚类。
- 前端：HomeView 的 `rows` 数组（`HomeView.vue:60`）加一行 `{ key: 'vod_hot', ... }`，
  复用 `MediaRow` 横滑，卡片角标「在线」。
- 空态：未配置采集源时不渲染该行（符合项目「拒绝生硬留白」的既定原则）。

#### P3-2 想看清单（护城河功能）

```sql
CREATE TABLE IF NOT EXISTS vod_wishlist (
    user_id     TEXT NOT NULL,
    clean_title TEXT NOT NULL,
    title       TEXT NOT NULL,
    year        INTEGER,
    kind        TEXT,
    poster      TEXT,
    status      TEXT NOT NULL DEFAULT 'waiting', -- waiting | online_found | local_found
    created_at  TEXT NOT NULL,
    notified_at TEXT,
    PRIMARY KEY (user_id, clean_title)
);
```

- 入口：搜索无本地结果时的空态上放「加入想看，有片源时通知我」。
- 后台任务（复用 `crates/task`）：每日轮询 wishlist → 对每个条目跑一次聚合搜索 +
  查本地库 → 命中则写 `notification` 表并更新 `status`（**双向**：在线源上线 / 本地入库）。
- 首页「想看清单」行展示 `waiting` 条目，命中后变为可播卡片。

**验收**：搜一部库里没有的片 → 加想看 → 手动触发任务 → 收到通知 → 首页清单条目变为可播。

---

### P4 · 工程打磨

- **接上已有缓存**：`repo::get/set_search_cache` 已实现却从未调用。
  在 `search_aggregate` 里按 `q` 的哈希接入，TTL 参考 MoonTV 的 7200s（影视元数据变化慢）。
  注意：缓存 key 要带上「启用的源集合」指纹，否则增删源后会命中旧结果。
- **消费 SSE**：`/search` 页改用 `?sse=true`，先渲染本地结果（毫秒级），
  再逐源追加在线结果，最后 `remote_cluster` 替换合并视图——避免首屏长时间空白。
- **搜索历史**：存 localStorage（对齐 MoonTV），chip 悬停删除。
- **源健康度面板**：设置页采集源表格增列「成功率 / 平均延迟 / 熔断状态」，
  数据来自 `CircuitBreaker` 的计数器。
- **清理任务**：`vod_entity` / `vod_search_cache` 过期清理，挂到现有 `reap-streams` 类任务上。

---

## 六、API 变更清单

| 方法 | 路径 | 阶段 | 说明 |
| :--- | :--- | :--- | :--- |
| GET | `/api/v1/search/aggregate` | P0 | 响应增 `merged` 字段（**向后兼容**） |
| GET | `/api/v1/vod/{key}` | P1 | 云端实体详情 |
| PUT | `/api/v1/vod/progress` | P0 | 在线播放进度上报 |
| GET | `/api/v1/vod/resume` | P0 | 在线续播列表 |
| POST | `/api/v1/vod/favorite` | P1 | 收藏在线条目 |
| GET | `/api/v1/vod/favorites` | P1 | 在线收藏列表 |
| DELETE | `/api/v1/vod/favorite/{key}` | P1 | 取消收藏 |
| GET | `/api/v1/vod/{key}/probe` | P2 | 线路探测评分 |
| GET | `/api/v1/vod/discover` | P3 | 全网热播发现 |
| POST | `/api/v1/vod/wishlist` | P3 | 加入想看 |
| GET | `/api/v1/vod/wishlist` | P3 | 想看清单 |

**门禁**：改完 Rust 接口必须跑 `make openapi` 同步 `docs/openapi/openapi.json`
与 `client/src/api/schema.d.ts`，`make openapi-check` 是 CI 门禁，不允许只改一边。

**儿童门禁**：以上新接口一律复用 `Current(principal).is_child()` 判定，
返回 `403 CHILD_RESTRICTED`，与 `proxy_vod_stream` 现有实现保持一致。

---

## 七、关键技术决策

### 7.1 为什么新增表而不是放宽外键

`playback_session.item_id` / `file_id` 与 `favorite.item_id` 都是 `NOT NULL` 外键。
放宽成可空看起来改动更小，但会污染两条已稳定的查询链路：

- 续播列表靠 `idx_session_user (user_id, updated_at DESC)` 与 `item_id` 联表取海报标题，
  掺进 `item_id IS NULL` 的行后，每个查询点都要加判空，漏一处就是 NPE 或空卡。
- 收藏计数、媒体库聚合统计同理。

**结论**：在线数据走独立表（`vod_progress` / `vod_favorite`），
在**应用层**做合并展示（首页继续观看行合并两个来源），物理表保持纯净。
代价是首页多一次查询，收益是不动任何既有 SQL。

### 7.2 同片合并放后端

聚类依赖 `TitleNormalizer`（正则清洗 + Levenshtein），前端没有等价实现。
放后端能保证本地与在线用**同一把尺子**比对，也避免 openapi 契约里暴露清洗规则。

### 7.3 云端实体 key 的稳定性

`vod_key = hash(clean_title + year)`，与 `AggregatedVodItem.id` 解耦：
后者是运行时派生、可能变，前者稳定可寻址、可收藏、可存进度。

### 7.4 TTL 6 小时

播放链接（尤其采集站的 m3u8）失效很快，缓存太久会给出一堆死链。
6 小时覆盖「今晚搜到、明晚继续看」的典型场景，过期后重新聚合一次即可。

### 7.5 合规

聚合源内容的版权风险由部署者承担。MoonTV 在 README 明确要求「设置密码、不公开分享、仅供个人使用」。
建议本项目同步：在设置页采集源区块加一行风险提示，且**默认不内置任何采集源**
（现状即是如此：`0009` 无种子数据，未导入则 `remote` 恒空——这个设计应保持）。

---

## 八、验收标准

每个阶段结束后跑：

```bash
cargo build --workspace --features openapi 2>&1 | grep -v "error deleting lock file" | grep -cE "^warning: [a-z]"
make openapi && make openapi-check
cd client && npm test
```

功能验收（手工冒烟）：

- [ ] P0-1 顶栏回车进 `/search?q=`，结果页三 Tab 正常
- [ ] P0-1 侧边栏「搜索」跳 `/search`（不再是死参数）
- [ ] P0-2 搜「奥本海默」只出现一张卡，同时标出本地与在线
- [ ] P0-3 在线播 30 秒退出，首页继续观看出现该片
- [ ] P1-1 `/vod/:key` 刷新后仍在，可分享
- [ ] P1-2 在线条目可收藏，出现在收藏页
- [ ] P1-3 本地详情页出现「在线线路」按钮
- [ ] P2-1 播放页可换源，进度不丢
- [ ] P2-2 换源列表按评分排序，死链源排最后且不报错
- [ ] P3-1 首页出现全网热播行；未配置源时不渲染
- [ ] P3-2 想看到货通知可触达
- [ ] 儿童账号访问上述新接口全部 403

---

## 九、排期建议

| 阶段 | 内容 | 相对规模 |
| :--- | :--- | :--- |
| P0 | 搜索结果页 + 同片合并 + 在线进度 | 小（后端只加字段和一张表） |
| P1 | 云端详情路由 + 收藏 + 在线补片 | 中（新增两个视图、两张表） |
| P2 | 换源侧栏 + 线路探测评分 | 中（涉及播放页，风险最高） |
| P3 | 全网热播 + 想看清单 | 中（含后台任务） |
| P4 | 缓存 / SSE / 历史 / 健康度 | 小（多为接线） |

**建议顺序**：P0 → P1 → P2 → P4 → P3。
P3 的想看清单依赖 P0/P1 的云端实体与通知体系，放最后收益最大；
若只想快速见效，**只做 P0 即可解决「搜索完就丢」和「重影」两个最刺眼的问题，
且完全复用现有 `search/aggregate` 接口，后端只需加一个 `merged` 字段。**

---

## 十、风险登记

| 风险 | 影响 | 缓解 |
| :--- | :--- | :--- |
| 采集站返回结构漂移 | 解析失败、`remote` 空 | `RawVodItem` 已做 string/number 兼容；单源失败被熔断器隔离 |
| 播放链接失效 | 用户点了播不了 | TTL 6h + 换源面板 + 播放失败时自动提示切下一条线路 |
| 缓存命中旧源集合 | 增删源后结果陈旧 | 缓存 key 带「启用源集合」指纹 |
| 儿童门禁遗漏新接口 | 合规风险 | 新接口统一走 `is_child()` 判定，纳入验收清单 |
| 播放页改造引入回归 | 本地播放受影响 | P2 改动严格限定在 `isVod` 分支内，本地链路零改动 |
| 法律合规 | 部署者风险 | 默认不内置源 + 设置页风险提示（对齐 MoonTV 做法） |
