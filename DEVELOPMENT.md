# seek — 开发记录

> 本文档记录**目的**、**最初的技术选型**、以及到目前为止**每一步做了什么**。
> 目的：让任何人（包括未来的我自己）都能看清"为什么这样做"，避免把核心做偏。

---

## 一、我们的目的

### 一句话

做一个 macOS 原生启动器，用 **fd 的遍历** + **fzf 的匹配与排序** 取代 Spotlight。

### 完整表述

> `seek` 是一个 macOS 原生 App。它把 `fd` 的文件系统遍历和 `fzf` 的模糊匹配
> 与排序规则装进一个悬浮面板里，取代 Spotlight。
>
> - **只搜索你指定的文件夹**（不像 Spotlight 索引整块磁盘）
> - **排序结果和 fzf 完全一致**（fzf 的排序很好，Spotlight 的不是）
> - **索引随文件系统实时更新**（启动后新建的文件立刻能被搜到）
> - **亚秒级响应**（按键到出结果，毫秒级）

### 成功的标准

> 敲一个键，正确的文件出现在列表顶部；再敲一个键就打开它。

### 谁在用

已经在终端里活着的开发者和重度用户。他们每天都在用 `fd` 和 `fzf`，知道 fzf 的排序好、
Spotlight 的不好。这个面板每天要开几百次，所以**出现必须瞬时**，**排序必须符合预期**。

### 不是什么

- 不是 WebView / Electron / Tauri 套壳（内存重、有 WebView 开销、观感不原生）
- 不是纯 Rust GUI（Tauri 要 Node，SwiftUI 才是 macOS 原生观感）
- 不是又一个"更聪明的搜索引擎"——**fzf 的排序就是契约**

---

## 二、最初的技术选型（计划书）

这是项目开始时定的方案。**下面标注了每一项的最终落地情况**，因为计划与现实
之间已经产生了若干偏差，而偏差本身需要被记录。

### 组件选型

| 组件 | 计划选型 | 理由 | 实际落地 |
| --- | --- | --- | --- |
| **核心语言** | Rust | 内存可控、单静态库嵌入 SwiftUI | ✅ 一致。`core/` 编译为 `staticlib`，Swift 通过 C ABI 调用 |
| **遍历 / 索引** | 借鉴 fd（`ignore`/`globset`/`walkdir`/`rayon`） | Apache-2.0 可商用 | ✅ 一致。用了 `ignore` 0.4 + `rayon`；`globset` 也在依赖里；`walkdir` 未直接用（`ignore` 已封装） |
| **模糊匹配** | `nucleo` v0.5.0 | Helix 同款，fzf 算法 | ⚠️ **改判**。改为**自己移植 fzf 的 `FuzzyMatchV2`** |
| **增量更新** | FSEvents（`fsevent-sys` 或 macOS 原生） | Cardinal 的 `cardinal-sdk` 可参考 | ✅ 一致。直接用 macOS 原生 CoreFoundation FSEvents（`core/src/watch.rs`），未引入 `fsevent-sys` crate |
| **UI** | SwiftUI（薄壳，通过 C ABI 调 Rust） | 原生观感 + 核心性能 | ✅ 一致 |

### 关键设计决策

**1. Rust 核心做成 C ABI 静态库，SwiftUI 只做视图。**
不要写纯 Rust GUI。Tauri 需要 Node 且内存更重。C ABI 导出 `extern "C"` 函数，
Swift 通过 module map 调用。这是内存最优解 —— 避免 WebView 的额外开销。
→ **已落地**：`core/src/ffi.rs`（979 行 C ABI），`core/include/seek.h`，
`app/Sources/Seek/SearchEngine.swift` 是 Swift 侧包装。

**2. 索引结构照搬 fd + Cardinal 的混合：**
- fd 的 `ignore` crate 做 gitignore 式排除 → ✅ 已落地（`core/src/scan.rs`）
- Cardinal 的 slab + interning 做紧凑存储（`SlabIndex` u32 + 名称池）
  → ✅ 已落地（`core/src/index.rs`：`NamePool` 字符串驻留 + `Vec<SlabIndex>` u32 寻址）
- 路径用 `Vec<u8>` 或 `Box<str>` 而非 `PathBuf`，省掉每条 24 字节结构体开销
  → ⚠️ **部分偏离**。索引条目存的是 `parent: Option<u32>` + `name: u32`（interned），
  **不是**每条一个 `PathBuf`，这一点符合计划；但 `Corpus` 为了给 matcher 喂连续
  字节，还是物化了一份 `Vec<String>` 路径。这是**刻意的取舍**（见下文"偏离原因"）。

**3. 模糊匹配必须放后台线程。**
nucleo 的设计就是后台线程池 + 快照发布。这和 Cardinal 的 search-cancel 版本化
取消机制可以对齐。
→ ✅ 已落地：搜索跑在 `Task.detached` 上，主 actor 只负责发布结果；
Rust 侧用 Rayon 并行评分 + `RwLock` 保护索引。

### 计划中两个"可以参考"的外部项目

- **Cardinal**（`github.com/cardisoft/cardinal`）—— 参考了它的 slab + interning 索引结构
  和 search-cancel 版本化取消机制
- **nucleo / Helix** —— 参考了它的后台线程 + 快照发布架构

---

## 三、计划的偏离与原因

计划的两个关键点被改判了。记录原因，避免以后又被改回去。

### 偏离 1：模糊匹配没有用 `nucleo`，而是移植了 fzf

**计划**：`nucleo` v0.5.0（Helix 同款，fzf 算法）。

**实际**：自己移植 fzf 的 `FuzzyMatchV2`（`core/src/fuzzy/`，1,822 行）。

**原因**：

1. **`nucleo` 的排序和 fzf 不完全一样。** nucleo 是"类 fzf"，不是 fzf。
   本项目的契约是 **fzf 的排序就是契约**（PRODUCT.md 产品原则第 2 条）。
   用 nucleo 就无法通过 fzf 的差分测试。
2. **差分测试是硬证据。** `core/tests/fzf_parity.rs` 直接调用真实的 `fzf --filter`
   二进制，对比匹配集合、分数和完整排序顺序。移植版能通过这个测试，
   nucleo 不能（因为它就不是 fzf 的算法）。
3. **可控性。** 自己移植可以精确控制内存布局、避免分配、并与 Rayon 切片配合。

**代价**：要自己维护 1,822 行的匹配器代码。**收益**：排序与 fzf 逐位一致，且可被测试证明。

### 偏离 2：Corpus 里物化了 `Vec<String>` 路径

**计划**：路径用 `Vec<u8>` 或 `Box<str>`，省掉每条 24 字节结构体开销。

**实际**：`Index` 里确实是紧凑的（`parent: u32` + `name: u32` interned）；
但 `Corpus` 另外物化了一份 `Vec<String>` 的相对路径。

**原因**：

1. **matcher 需要连续的字节。** fzf 的 `FuzzyMatchV2` 在一段连续的字节上跑
   动态规划。`Vec<String>` 提供了连续的、可切片给 Rayon 的内存。
2. **"搜索覆盖整条路径"是产品要求。** fzf 匹配的是整条路径而不是文件名，
   所以 haystack 里必须有分隔符，路径感知的评分方案才有意义。
3. **换来的收益是可测的。** `core/examples/bench.rs` 实测：233,505 个文件、
   258,376 条索引下，搜索 21–69 ms，真实 `fzf` 是 51–217 ms（且 fzf 还要算
   进程启动）。

**结论**：这是有意识的取舍，不是疏漏。索引本身仍然是紧凑的。

---

## 四、到目前为止的每一步

### 第 0 步：研究参考项目

**做了什么**：读了 `github.com/cardisoft/cardinal`，确认了它的 slab + interning
索引结构和版本化取消机制值得借鉴。

**产出**：技术选型表（本文档第二节）。

---

### 第 1 步：Rust 核心 —— fd 式遍历 + fzf 式匹配

**做了什么**

| 文件 | 行数 | 作用 |
| --- | --- | --- |
| `core/src/fuzzy/matcher.rs` | 684 | fzf `FuzzyMatchV2` 移植：DP 评分矩阵、字符分类、bonus 矩阵、重音折叠 |
| `core/src/fuzzy/score.rs` | 244 | 评分矩阵 |
| `core/src/fuzzy/rank.rs` | 239 | fzf 的 tiebreak 链：score → length → chunk → pathname → begin → end |
| `core/src/fuzzy/normalize.rs` | 423 | 拉丁重音折叠 |
| `core/src/fuzzy/scratch.rs` | 65 | 可复用的 DP 矩阵缓冲（零分配） |
| `core/src/scan.rs` | 416 | fd 式并行遍历（`ignore` crate） |
| `core/src/index.rs` | 530 | slab + interning 索引 |
| `core/src/corpus.rs` | 123 | 可搜索的扁平结构 |
| `core/src/search.rs` | 234 | Rayon 并行搜索 |
| `core/src/query.rs` | 291 | fzf 查询语法（`'exact'` `^prefix` `suffix$` `!exclude`） |
| `core/src/persist.rs` | 278 | postcard + zstd 快照 |
| `core/src/ffi.rs` | 979 | C ABI |

**关键决策**

- 排序严格复刻 fzf 的 tiebreak 链，**不是**"更好"的排序
- haystack 是**相对根目录的整条路径**（fd 打印的东西），不是文件名
- 索引用 `parent: u32` + `name: u32`（interned），路径不重复存储

**验证**：`core/tests/fzf_parity.rs` 差分测试对比真实 `fzf` 二进制 ——
匹配集合、分数、完整排序顺序全部一致。

**产出**：commit `1961118`（8,148 行）

---

### 第 2 步：SwiftUI 面板 + C ABI 桥接

**做了什么**

| 文件 | 作用 |
| --- | --- |
| `app/Sources/Seek/SearchEngine.swift` | Swift 包装 Rust 引擎 |
| `app/Sources/Seek/AppModel.swift` | 状态机：查询、结果、索引生命周期 |
| `app/Sources/Seek/ContentView.swift` | 悬浮面板布局 |
| `app/Sources/Seek/ResultRow.swift` | 结果行（fzf 式高亮） |
| `app/Sources/Seek/SeekApp.swift` | 应用入口、面板、热键 |
| `app/Sources/Seek/PreferencesView.swift` | 设置窗口 |
| `app/Sources/Seek/HotKeyManager.swift` | Carbon 全局热键 |
| `app/Sources/Seek/Theme.swift` | 视觉规范 |

**关键决策**

- **C ABI 静态库**，Swift 只做视图 —— 遵循原计划
- 热键用 Carbon `RegisterEventHotKey`，**不需要辅助功能权限**（全局事件监视器需要）
- 面板用 `NSPanel` + `NSVisualEffectView` vibrancy，`LSUIElement` 无 Dock 图标
- 面板自己的键用**本地事件监视器**拦截，不��背景 `NSView`——
  背景视图不在 responder chain 里，压根收不到按键

**产出**：commit `1961118`

---

### 第 3 步：FSEvents 增量更新

**做了什么**：`core/src/watch.rs`（560 行），直接用 macOS 原生 CoreFoundation
FSEvents。索引在扫描后保持实时：启动后新建的文件立刻能被搜到。

**验证**：`core/tests/live_index.rs`、`core/tests/watcher.rs`

**产出**：commit `4771ddf`

---

### 第 4 步：修 UI（第一轮）

**做了什么**：用户反馈 "the ui too ugly"。

- 新建 `Theme.swift`：连续圆角（`PanelShape` / `RowShape`）、SF Pro 字阶、
  统一语义色、键盘快捷键徽章
- 面板背景从 `.hudWindow` 改为 `.sidebar` vibrancy（对比度更好）
- 结果行：图标、文件名/目录的层级、匹配字符高亮
- 设置窗口：分区标题、说明文案、统一的间距

---

### 第 5 步：修"输入没有结果"（关键 bug）

**症状**：用户输入任何字符，搜索结果都是 0 条。

**根因**（两层）：

1. **任务取消把结果一起扔了。** 文件监视器每 400ms 触发一次，会取消正在进行的
   搜索任务。而 `await Task.detached { }.value` 在父任务被取消时抛 `CancellationError`，
   结果还没发布就被丢弃了 —— 面板永远空白。
   **修法**：把"用户查询"和"后台索引更新"两条路径分开；不再取消进行中的搜索，
   改用单调递增的 `generation` 计数器丢弃过期结果。

2. **每条结果一次 FFI 调用。** `sk_engine_path` / `sk_engine_relative_path` /
   `sk_engine_positions` 各自取一次读锁，500 条结果 = 1,700 次加锁 + 500 次
   `FileManager` 磁盘 stat。一次查询 1.7 秒。
   **修法**：新增批量 FFI `sk_engine_resolve`（一次加锁，路径写进连续缓冲区 +
   偏移索引）和 `sk_engine_positions_batch`（一次加锁，一次解析查询）；
   `is_dir` 直接加进 Rust 的 `SearchResultEntry`，消灭 500 次 stat。

**结果**：后处理从 1,700ms → 毫秒级。

**验证**：`SearchSchedulingTests`、`WatcherSearchCancellationTests`

---

### 第 6 步：修"面板根本不显示"（今天的关键 bug）

**症状**：面板窗口存在但 `onscreen=false`，用户看不到任何东西。

**根因**：`hidesOnDeactivate = true` 与 `NSApp.activate()` 的时序冲突。

`showPanel()` 原本是：

```swift
panel.makeKeyAndOrderFront(nil)   // 先显示
NSApp.activate(ignoringOtherApps: true)  // 后激活
```

`hidesOnDeactivate` 会在**任何**失去激活状态时隐藏面板 —— 而后面那句
`activate` 正好触发了前面那次显示的"失去激活"，面板刚显示就被隐藏。
用户看到的是：什么都没有。

**修法**：

```swift
NSApp.activate(ignoringOtherApps: true)  // 先激活
panel.makeKeyAndOrderFront(nil)           // 后显示
```

并且 `hidesOnDeactivate` 改为 `false`，由 `applicationDidResignActive` 显式隐藏
（带 `isOpeningPanel` 守卫，避免启动时的激活把刚开的面板关掉）。

**顺带修的一个隐蔽 bug**：`makePanel()` 里 `NSHostingController` 是局部变量，
函数返回后就被释放了。`NSHostingController` 持有连接 model 和 view 的订阅，
被释放后**面板不再更新** —— 查询能到 model，model 能找到文件，但结果永远画不出来。
改成 `private var panelHosting: NSHostingController<AnyView>?` 持有。

---

### 第 7 步：把"fd 和 fzf 是核心"变成硬约束

**问题**：这个约束只写在 `PRODUCT.md` 的原则里，是文档不是约束。

**做了什么**

1. 新建 `.cursor/rules/fd-and-fzf-are-the-core.mdc`（`alwaysApply: true`）
   —— 把约束写成规则，包括：什么算"改进"、什么不算、
   改匹配器必须跑差分测试、哪些文件是核心。
2. **补上 `fd` 的差分测试**（之前只有 fzf 有）：`core/tests/fd_parity.rs`，
   4 个测试对比真实 `fd` 二进制：
   - 找到的路径集合完全一致
   - 隐藏文件都跳过
   - `.gitignore` / `.fdignore` 都遵守
   - `--exclude` 模式效果一致
3. 修好了两个因为给 `SearchResultEntry` 加 `is_dir` 而**编译失败**的 core 测试
   （`ffi_scan.rs`、`concurrency.rs`、`live_index.rs`）—— 这些测试之前一直是红的。

**验证**：

```
cargo test                            → 28 + 8 + 5 + 2 + 1 + 1 + 1 全过
cargo test --test fzf_parity -- --ignored  → 3 passed
cargo test --test fd_parity  -- --ignored  → 4 passed
```

---

### 第 8 步：活体验证（`--query` / `--snapshot`）

**问题**：测试能证明 model 里有结果，但证明不了**屏幕上画出来了**。
这两件事不一样 —— 而用户看到的恰恰是后者。

**做了什么**：给 App 加了两个命令行开关：

| 开关 | 作用 |
| --- | --- |
| `--query=readme` | 启动后自动打开面板并输入这个查询 |
| `--snapshot=/tmp/x.png` | 3 秒后把**活的 panel view** 渲染成 PNG |

**为什么用"活的 view"而不是重建一个视图**：重建的视图走的是测试路径，
而 `--snapshot` 走的是真实 panel 的 `contentView`，
它渲染出来的就是用户眼睛看到的那棵树。

**顺带修了一个 bug**：`showPanel()` 会调 `model.prepareForOpen()` 清空查询，
所以 `--query` 一设就被抹掉了。修法是等 panel 显示完再设查询。

**验证**：活体快照确认 —— 查询词保留、结果按 fzf 排序、匹配字符高亮、
目录分隔线渲染。

![活体面板快照](/tmp/l2.jpg)

---

### 第 9 步：修"面板一直是空的"（真正的根因：锁饥饿）

**症状**：索引里有 258,266 个条目，输入 `readme`，`model.query` 确实变成了
`readme`，面板却什么都不显示，状态栏停在 `258266 files indexed`。
`PanelTypingSearchTests` 的两个用例复现了它。

**错误的怀疑**：第 5 步和第 6 步都把这个症状归到了 Swift 侧（任务取消、
面板时序）。都不是。

**怎么定位的**：在 `AppModel` 的每个阶段打日志，看它停在哪一步。

```
DBG schedule q="readme" gen=2
DBG slept   gen=2 cancelled=false
（没有下文）
```

`engine.search()` 根本没有返回。加上计时之后：

```
DBG poll blocked ms=1348
DBG save done   ms=428
DBG searched gen=2 hits=200 ms=6083   ← 搜索被阻塞了 6 秒
```

**根因**：引擎把**索引**和**语料**放在同一把 `RwLock` 下。

- `sk_engine_poll_changes` 每 400 ms 取一次**写锁** —— 而它只是把 watcher
  的 channel 抽干，根本不需要写权限。
- `sk_engine_apply_changes` 拿着写锁做 `fs::metadata` 系统调用，然后对
  **全部 25 万条目**重建 `Corpus`（每条都要回溯父链重算路径）。
- `sk_engine_search` 要读锁。

macOS 的 `std::sync::RwLock` **偏向写者**：只要有一个写者在排队，后面所有
读者都得等。于是 watcher 每 400 ms 排一次队，每次键入都被压在后面几秒。
面板不是"不显示结果"，而是**搜索压根没返回**。

**证据**：`core/tests/search_under_maintenance.rs` 在持有维护锁的情况下发起
搜索。修复前这个测试**永远挂住**（不是变慢，是死等）。

**修法**：把可搜索状态作为**不可变快照**发布出去 —— 也就是需求文档
第 4.5 节里本来就写了的 Helix/nucleo "快照式发布结果"模式。

```rust
pub struct Engine {
    inner:   RwLock<Inner>,        // 索引、scanner、watcher：维护用
    corpus:  RwLock<Arc<Corpus>>,  // 发布出去的可搜索快照
    options: RwLock<SearchOptions>,
}
```

- 搜索克隆一个 `Arc`（纳秒级）然后**在所有锁之外**跑完。
- `Corpus` 自带搜索和显示需要的一切（`is_dir`、所属 root、相对路径），
  所以搜索再也不用回头查索引。
- 维护照常拿写锁、想拿多久拿多久，改完再原子换上新快照。
- `poll_changes` 改成读锁，`metadata` 系统调用挪到锁外。

**顺带修掉的**：

| 问题 | 说明 |
| --- | --- |
| `app_path_parity` 硬编码 `/Users/bioguo/projects` | 换成自己生成的树，任何机器上都能跑 |
| 同一个测试把截断到 500 条的结果和 fzf 的全量结果比"集合" | 比集合时把 limit 抬到语料之上 |
| 同一个测试拿我们的 Path scheme 比 fzf 的 default scheme | 给 fzf 传 `--scheme=path --tiebreak=length`（`--scheme=path` 会偷偷改掉 fzf 的默认 tiebreak） |
| `fzf_parity` 的语料取自某台机器的 home | 改成以仓库自身为根 |
| 部署目标不匹配 | `core/.cargo/config.toml` 钉了 14.0，但 cargo 按**工作目录**读配置，而脚本是从仓库根目录带 `--manifest-path` 调用的 —— 配置从没生效，产物 `minos 26.5`，而 Info.plist 写的是 14.0。改成在脚本里导出 |
| `PanelTypingSearchTests` 只断言"结果非空" | 空查询匹配一切，所以上一次查询的残留永远非空 —— 这个断言在显示错误结果时照样通过。改成断言每一行都真的匹配所输入的查询，并且状态栏与列表一致 |
| 没有任何测试断言结果被**画**出来 | 新增 `PanelDrawsResultsTests`：渲染面板，数结果区的像素 |

**验证**：

- `search_under_maintenance` 从"永远挂住"变成通过
- `PanelTypingSearchTests` 两个用例通过，状态栏为 `200 of 258268 files`
  （第 15 步改成了 `47,067 matches · showing 200`，见下文）
- watcher 跑着的时候搜索 24–52 ms（原来是几秒）
- fd / fzf 两组差分测试全部仍然通过

---

### 第 10 步：对齐 fzf 的 path scheme（两个藏在"匹配位置"里的 bug）

**起点**：app 用 `Scheme::Path`，但 tiebreak 链还停在 `score → length`。
`man fzf` 一句话就说清了：`--scheme=path` 自己会设 `--tiebreak=pathname,length`。
按 F3 的说法，排序和 fzf 不一致就是 bug。

把链条挂到 scheme 上（`Scheme::criteria`）之后，差分测试没有变绿，只是
分歧点往后挪了。接下来挖出来的两个问题，都不在"排序规则"这一层，而在
**匹配位置**这一层 —— 排序规则再对，量错了地方也白搭。

**bug 1：扫描方向**

`libparse/src/main.rs` 里搜 `a`，`libparse` 的 `a` 和 `main.rs` 的 `a`
分数一模一样。报哪一个，决定了 `pathname` 把这条算成目录命中（排在所有
文件名命中之后）还是文件名命中。

fzf 在 `core.go` 里按 tiebreak 链选方向：

```go
case byEnd:      forward = false
case byBegin:    forward = true
case byPathname: withPos = true; forward = false
```

反向扫描时 fzf 把 `score > maxScore` 换成 `score >= maxScore`，也就是
同分取最后一个。这件事一开始很难看出来，因为单独测 `--tiebreak=begin`
时 fzf 报的起点是 4（`libparse` 里的 `a`），而 `--scheme=path` 下的排序
表现得像起点是 14 —— 看着自相矛盾，其实是**两次测量用了不同的方向**。

顺带修了单字符快路径的一个偏差：fzf 的"遇到边界 bonus 就停"是写在
"这一次确实刷新了最高分"的分支**里面**的，我们写在了外面。一个分数更低
的边界命中会让我们提前停下，而 fzf 会继续扫。

**bug 2：匹配起点根本就不是匹配起点**

`libparse/src` 搜 `src`：`src` 可以从 `libparse` 的 `s` 开始（第 6 位，
后面还能找到 `r` 和 `c`），也可以就是分隔符后面那个真正的 `src`（第 9 位）。
我们报的是 `f0` —— 首字符**可能**落脚的第一个位置，不是 DP 真正选中的路径
的起点。所有基于 span 的 tiebreak 都是从起点量的，于是文件名命中又一次被
算成了目录命中。

而且报出来的分数和报出来的起点本身就对不上：分数 27 对应的是第 17 位的 `t`，
起点却报 16。这就是 bug 的指纹 —— DP 算对了，回溯没取回来。

fzf 的 backtrace 会给出真正的起点，并且正因为链里需要 span 时才需要它，
fzf 才会在 `byChunk` / `byPathname` 时打开 `withPos`。现在起点一律取自
backtrace，写进可复用的 scratch buffer，所以每条候选都跑也不分配内存
（实测只贵 5–13%）。

**差分测试本身也是错的**

原来逐位比 `fzf --filter` 的输出。但输入一大，这其实是在比 fzf 的
**并行分块合并**，不是在比排序：

- 把 `fzf --filter` 的输出再喂回 fzf，条目会挪位（不是幂等的）；
- 单独问它那两条谁在前，它给的答案和自己输出里的顺序相反。

同一份输入下，我们的 372 条结果按 key 零逆序，fzf 的输出有 1 处逆序。
所以现在的做法是：匹配**集合**必须完全一致；顺序上，只要 app 的 key 和
fzf 打印的顺序冲突，就把这两条**单独**交给 fzf 裁决 —— 那是 fzf 唯一
精确的模式。pattern 也从 19 个扩到 35 个，专门加了能同时命中目录和文件名的
短模糊串，因为 `src` 这个案例是撞上的，不是测出来的。

**顺带修的性能问题**

查这个问题时发现 `search.rs` 的注释写着"每个 worker 一个 scratch buffer"，
代码却是在 per-candidate 的闭包里 `Scratch::new()` —— 25.8 万次/击键。
REQUIREMENTS.md 里"零分配匹配器"那句话当时是不成立的。改成 `map_init` 后
（注意要用 `filter_map` 收尾，`flatten` 会把每条候选变成一个单元素迭代器，
实测慢两成），再把 `pathname_score` 里每条候选一次的 `Vec<char>` 分配去掉：

| 查询 | 之前 | 现在 | 同机 fzf |
| --- | --- | --- | --- |
| `src` | 75 ms | **50 ms** | 169 ms |
| `test` | 97 ms | **69 ms** | 223 ms |
| `s` | 52 ms | **41 ms** | 242 ms |
| `index` | 30 ms | **23 ms** | 54 ms |

### 第 11 步：性能优化（先量，再改）

第 10 步收尾时 4 字母查询 69 ms，还在 F3 的 50 ms 预算之上。这次先用
`sample(1)` 打了个 profile，而不是按直觉猜 —— 结果猜错了两处：

**改 1：`fold` 的 ASCII 快路径**（69 → 58 ms）

profile 里 `core::unicode::unicode_data::conversions::to_lower` 赫然在列。
`fold()` 对非大写字符走的是 `c.to_lowercase().next()`，而这是个迭代器，
要为**每个字符**构造一次。路径里绝大多数字符是 ASCII 小写，正好全都走这条。
`normalize_char` 对 U+00C0 以下本来就直接返回 `None`，所以 ASCII 整条路
可以缩成一次比较加一次条件减法，语义完全不变。

**改 2：prefilter 用 `memchr`**（58 → 50 ms）

`fuzzy_index` 要正向扫一遍找每个 pattern 字符，再反向扫一遍找最后一个 ——
每条候选两趟全长扫描，逐字符 `fold` 比较。`memchr` 本来就是依赖里的。
给 `CharSource` 加了个 `ascii_bytes()`，纯 ASCII 的源返回字节切片，
`find_char` / `rfind_char` 就能用 `memchr2` 向量化（大小写两个字节一起找）。
非 ASCII 的源照旧逐字符走。

**改 3：只排前 `limit` 条**（50 → 35 ms，最大的一笔）

profile 里 `quicksort` + `driftsort` 占了一个 worker 近两成时间。原因很蠢：
面板只显示 500 行，我们却把十万条全排了。

不能直接换成 `select_nth_unstable` —— 那会打乱并列项的顺序，而并列顺序
是靠稳定排序维持的。但 fzf 本来就在每条链末尾隐式追加 `index`（输入顺序），
把它**显式写出来**之后比较就成了全序，没有并列可打乱了，于是可以先
`select_nth_unstable_by` 切出前 `limit` 条再排。顺带这也让"靠排序稳定性"
这件隐含的事变成了代码里写明白的事。

加了 `a_capped_search_is_the_top_of_an_uncapped_one` 守着：截断后的结果
必须和不截断时的前缀逐条相同。把显式的 index tiebreak 去掉，这个测试会红。

**改 4：`trim_length` 缓存到 corpus**（35 → 33 ms）

它是 tiebreak 的一个 key，但**不依赖查询** —— 每次击键为 25 万条候选重算
一遍纯属浪费。fzf 自己就是把 `trimLength` 缓存在 item 上的
（`chars.trimLengthKnown`），所以缓存在 corpus 上是照抄 fzf，不是取巧。

顺手修了一个对齐 bug：fzf 的 `TrimLength` 是**两端**都 trim（`i - j + 1`），
我们只 trim 了尾部。

**结果**（25.8 万条路径，同机 fzf 对照）

| 查询 | 第 10 步后 | 现在 | fzf | 对 fzf |
| --- | --- | --- | --- | --- |
| `src` | 50 ms | **29.3 ms** | 144 ms | 4.9× |
| `test` | 69 ms | **43.4 ms** | 186 ms | 4.3× |
| `s` | 41 ms | **16.9 ms** | 189 ms | 11.2× |
| `cargo` | 28 ms | **17.0 ms** | 54 ms | 3.2× |
| `index` | 23 ms | **12.6 ms** | 44 ms | 3.5× |
| `seek` | 7.4 ms | **1.6 ms** | 2.7 ms | 1.7× |

全部进了 50 ms 预算。峰值 RSS 也从 260 MB 降到 221 MB（少了大量临时分配）。

---

### 第 12 步：UI 精修（四个缺陷 + 一项密度）

截图看出来的，不是想出来的。先渲染三张快照（空状态 / 有结果 / 无匹配），
对着截图找问题，再回代码里找根因。

**缺陷 1：空状态第一行是空白的，而且被选中。**

根因在 core，不在 UI。`Corpus::build` 对每个条目调 `relative_path_with_root`，
而 root 相对自己的路径是空串。空串的 `trim_length()` 是 0，fzf 的 `Length`
tiebreak 在分数相等时按长度排 —— 空查询下所有条目分数都是 0 —— 于是 root
排第一；Swift 侧 `lastPathComponent` 对空串给空串，画出来就是一行什么都没有。

修法是不让它进 corpus：

```rust
if path.is_empty() {
    continue;
}
```

在 corpus 里过滤，而不是在搜索或 UI 里过滤，这样"corpus 里的候选一定有文本"
是一条在一个地方就能读到的事实。

**缺陷 2：空查询返回的是"路径最短的前 N 个"。**

引擎对空模式的回答和 fzf 一样 —— 全部命中 —— 所以空查询会扫 25 万条路径、
按长度排序，然后显示一串用户没问过的顶层目录。引擎没错，是面板问错了问题。
空查询在 `AppModel.scheduleSearch` 里直接发布空结果，连扫都不扫。

这条分界线值得写下来：**引擎的行为跟 fzf 对齐，启动器的行为跟启动器对齐。**
空模式匹配一切是 fzf 的语义，不是 launcher 的语义。

**缺陷 3：面板高度是写死的 460pt。**

空状态和无匹配状态是一行字放在一个大空盒子里，而盒子就是用户看到的东西 ——
面板的背景和边框都是 `ContentView` 自己画的。改成窗口跟着内容走：
`ContentView` 用 `onGeometryChange` 报自己的高度，`AppDelegate.resizePanel`
接住。

两个坑：

- `ScrollView` 没有自己的高度（它要多少给多少），所以要量的是它*里面*的
  `LazyVStack`，再把 `min(量到的, Theme.maxResultsHeight)` 给回去。
- NSWindow 的 origin 是**左下角**。保持 origin 不动而改高度，会把搜索框随着
  结果到达上下拖动。所以 `origin.y -= delta`，钉住上边缘；`centerPanelOnMouseScreen`
  也改成按上边缘定位（用 `panelMaxHeight` 算，这样空面板和满面板的搜索框在
  同一个位置）。

还有一个只有真跑才知道的风险：如果"内容高度"和"窗口 frame 高度"量的不是同一
个东西（差一个标题栏就够了），每次 resize 都会触发下一次，面板永远停不下来。
`test_the_real_panel_follows_its_content` 驱动真实 delegate，隔 700ms 采样两次
断言高度相等，就是守这个的。

**缺陷 4：无匹配时正文和 footer 说同一句话。**

`updateStatus` 在无结果时不再重复 "No matches for ..."，改报索引规模 ——
footer 说只有它知道的事：搜了多少东西才没搜到。

**密度：** 行的 `padding(.vertical)` 5 → 4。

**`Theme.maxResultsHeight = 363` 是个写死的数**，等于 460 减去搜索框、footer
和两条分隔线占的 97pt。写死而不是量，因为这三样都不随运行时变化；代价是改了
padding 它就会漂。`PanelHeightTests` 从两边夹住它：太大则面板超出天花板，太小
则永远够不着天花板。把它改成 500 验证过第一条确实会红（597 > 460）。

测试：core 35 unit（新增 2 个 corpus），app 51（新增 4 个 `PanelHeightTests`
+ 1 个真实面板 resize）。两个新断言都先验证过能红：去掉 `path.is_empty()`
的守卫，corpus 测试给出 `left: ["", "main.rs"]` —— 正是那行空白；去掉
`origin.y -= delta`，上边缘断言给出 609 ≠ 629。

---

### 第 13 步：glob 和 regex（以及查询语言里三个潜伏的 bug）

起点是一句话："试试 `*.pdf`"。它返回 0 结果 —— `*` 不是任何路径里的字符，
模糊匹配器挨个拒绝。但顺着这条线写了个探针跑真实查询路径，翻出来的比预期多：

| 输入 | 改之前 |
|---|---|
| `*.pdf` | 0 命中 |
| `^Docs` | 0 命中（而 `docs/` 就在那儿，搜索还是大小写不敏感的） |
| `pdf$` | 漏掉 `Test_Report.PDF` |
| `'pdf'` | 9 命中，含 `papers/draft.tex` |
| `content:hello` | **整个索引** |

后三行是没人问起的 bug，根因各不相同但都在同一段代码里：

**`^`/`$`/`'...'` 是字节精确的。** search.rs 用裸的 `starts_with` / `ends_with`
/ `contains`，不看 `case_sensitive`。于是同一个搜索框里有两套大小写规则：模糊
匹配折叠大小写，这三个操作符不折叠。修法是 `fuzzy/literal.rs` —— 三个带
`case_sensitive` 参数的谓词，ASCII 走 `eq_ignore_ascii_case` 加 memchr 跳过
（路径几乎都是 ASCII，而 `contains` 要在 25 万条上跑），非 ASCII 走
`char::to_lowercase` 折叠后比较。

**`'foo'` 从来没产生过 `TermKind::Exact`。** 引号只负责"让词里能带空格"，
`kind` 仍是 `Fuzzy`。`TermKind::Exact` 这个枚举值在整个代码库里从未被构造过，
而模块文档写着"精确子串"。现在引号产生真正的 Exact 词，并且**引号内全是字面
量** —— `'^foo'` 里的 `^` 是字符不是操作符，这样"引号 = 这些确切的字符"才是一
句完整的话。

**`content:` 被静默丢弃。** 它解析成 `TermKind::Content` 进 `positive`，而
search.rs 的过滤器没有一个认这个 kind，于是整条词蒸发，查询退化成"匹配一切"。
这比不支持更糟 —— 它用"全部"回答了一个它读不懂的问题。现在解析时就报
`Unsupported`。

**新增 glob 和 regex。**

glob 不要新语法：词里有 `*` 或 `?` 就是 glob。`fd` 需要 `-g` 是因为它默认是
regex；这里默认是模糊匹配，而 `*` 和 `?` 不出现在真实路径里，所以它们的出现本
身就是无歧义的信号。regex 必须显式标记 `re:`，因为 regex 语法由文件名里再普通
不过的字符组成 —— 单是 `.` 就会把每个查询变成 regex。`re:` 复用了语法里已有的
`content:` 那个形状。

两者都匹配**相对路径** —— 和模糊匹配器打分的、面板显示的是同一段文字。没有
`/` 的 glob 会被加上隐式 `**/`，所以 `*.pdf` 找得到 `docs/report.pdf`；有 `/`
的按字面匹配，配 `literal_separator(true)` 让 `*` 停在目录边界。`globset` 和
`regex` 本来就在 `Cargo.toml` 里躺着没被用过。

**`!` 现在带着被它否定的那个词的 kind。** 以前所有负向词都当模糊匹配，于是
`!*.tmp` 排除的是"按顺序含 t、m、p 的路径"，和 `*.tmp` 排除的完全是两个集合。

**非法模式是错误，不是空列表。** 解析时就编译 glob 和 regex，失败则整条查询
报错。FFI 新增无状态的 `sk_query_error`（不拿锁，不碰引擎，所以每次按键问它都
不花钱），面板用它把"那不是一个合法的正则"和"没有这个文件"分开。顺带修了
`ffi.rs` 里的 `Err(_) => Query::default()` —— 默认 Query 是*空*查询，匹配一
切，所以一个写坏的 `re:[` 过去会把整个索引铺满屏幕并称之为搜索。

测试：`fuzzy::literal` 7 个、`query` 20 个、`tests/query_language.rs` 12 个
端到端（走真实 corpus 和真实 search），app 侧 `QueryErrorTests` 5 个。

### 第 14 步：应用图标

项目此前**没有图标** —— 没有 `.icns`，`Info.plist` 里没有 `CFBundleIconFile`，
bundle 出来的 app 用 macOS 的默认空白图标。

图标是**代码**（`tools/make-icon.swift`）而不是仓库里的一张图：改比例改配色是
一次有 diff 的编辑，而且每个尺寸在自己的尺寸上绘制，不是从一张大图重采样下来。

这件事有实际回报。标记是"排序结果 + 高亮匹配"：三条逐渐变窄的行，每行开头是蓝
色的匹配段 —— 画的就是面板真正显示的东西，比再来一个放大镜强。但 16pt 下三条
行每条不到一个像素高、行距也糊在一起，整个标记变成一团灰。所以 32pt 以下换成
两条更粗的行，说同一件事而且活得下来。这正是"每个尺寸各自绘制"买到的东西。

`bundle.sh` 现在生成 iconset、`iconutil` 打包、拷进 bundle，然后才签名。

---

## 五、当前状态

### 已验证可用

- ✅ 搜索内核：fzf 排序逐位一致（差分测试）
- ✅ 遍历：fd 行为一致（差分测试）
- ✅ 增量更新：FSEvents 实时
- ✅ 性能：25.8 万条路径下 **1.6–43 ms**，全部在 50 ms 预算内
  （同机 fzf 2.7–189 ms，快 1.7–11 倍）
- ✅ 内存：常驻 60.8 MB / 25.8 万条（索引 18.4 MB + corpus 42.4 MB）
- ✅ fzf 的 path scheme 逐位一致（扫描方向 + 真实匹配起点，见第 10 步）
- ✅ 面板显示：已修复（`hidesOnDeactivate` 时序 + `NSHostingController` 生命周期）
- ✅ 端到端：`AppDelegateIntegrationTests` 走真实 delegate，真实按键，得到真实结果
- ✅ 活体截图：`--snapshot` 渲染真实 panel，确认结果画在屏幕上
- ✅ **面板真的显示结果**：`PanelDrawsResultsTests` 渲染面板并数结果区的像素，
  `PanelTypingSearchTests` 断言每一行都匹配所输入的查询（见第 9 步）
- ✅ 搜索不再被索引维护阻塞：`search_under_maintenance` 在持有维护锁时断言搜索仍有应答
- ✅ 部署目标与 Info.plist 一致（`minos 14.0`）

### 测试

```
core:  78 个（54 unit + 12 query-language 端到端 + 9 integration + 5 path
       + 2 scanner + 1 ffi + 1 live + 1 watcher
       + 1 maintenance 搜索不被维护阻塞 + 1 concurrency）
       + 8 个差分：3 fzf parity + 4 fd parity + 1 app path parity
                  （差分测试要真实的 fd / fzf 二进制，用 --ignored 跑）
app:   56 Swift tests（含端到端：真实面板按键 / 真实 delegate / 画面像素
       / 真实窗口跟随内容改高度 / 非法查询的报错）
```

## 第 15 步：footer 报匹配数，而不是行数

**症状**（用户报的）：第一次检索有结果，之后删字符列表不跟着变。

**先纠正一个测量错误。** 我第一次量到"删 3 个字符要 639ms"，那个数是假的：
`swift test` 不带 `SEEK_CORE_DIR` 时链接的是 `core/target/debug`，一个
未优化的 Rust core。换成 app 实际打包的 release core 重测，同一台机器：

| 测量项 | debug core | release core |
|---|---|---|
| `engine.search("rea")` | 252 ms | **30 ms** |
| 删 3 个字符到列表更新 | 639 ms | **46 ms** |

所以不是性能问题，也不是 task 取消／watcher 抢锁的问题 —— `sk_engine_search`
只拿 `corpus.read()`，和 `sk_engine_apply_changes` 的维护锁完全分开，键盘搜索
从不排在 watcher 后面。

**真正的根因。** 面板最多显示 `limit`（200）行。在 25 万条目的 home 目录上：

```
"readme"  → 真实匹配 1281 个，显示 200 个
"readm"   → 真实匹配 1288 个，显示 200 个
top-200   → 完全相同
footer    → 两次都是 "200 of 258328 files"
```

拿同一份候选列表独立问了 fzf（`fzf --filter --scheme=path`，66 万条路径）：
**fzf 自己给出的 top-200 对这两个查询也完全相同**。按
`fd-and-fzf-are-the-core`，排序和 fzf 一致就不是 bug。

所以缺陷在 footer：`"\(results.count) of \(indexedCount) files"` 把
**shown** 放在了 fzf 计数器放 **matched** 的位置。`200` 不是事实，是缓冲区
容量。删掉一个字符，可见的那 200 行和 footer 文字一起保持字节级相同 —— 一个
正确的搜索看起来像卡死的面板。这也解释了"第一次检索可以有结果"：第一次是
从空列表变成有内容，变化是可见的。

**改动。**

- `SearchOutcome` 加 `matched` 字段，在 `select_nth_unstable_by` 截断**之前**
  记下 `candidates.len()`。零额外开销：候选那时已经完全物化了
- `sk_engine_search` 加可空出参 `int32_t *matched`。5 个 Rust 调用点传
  `null_mut()`；出错和查询解析失败时写 0，这样调用方不会读到上一次的残值
- Swift 的 `search` 返回 `SearchOutcome { hits, matched }`，`AppModel` 多一个
  `matchedCount`
- footer 改成 `1,281 matches · showing 200`，只在真被截断时才加后半句；
  一个匹配时用单数 `1 match`

**fzf 不截断**，它保留全部匹配让你滚动；我们截到 200 是 app 自己的决定
（`search.rs` 的注释写明先截断再排序是匹配之外最大的性能来源）。所以我们比
fzf 多一个要表达的事实，footer 用了散文而不是 fzf 那种 `1281/258328`。

**测试：**

- `core/tests/matched_count.rs` 5 个用例：计数忽略 limit、不截断时等于行数、
  无匹配为 0、不计被过滤器丢掉的、以及缩短查询时行数不变而计数变
- `app/Tests/.../FooterReportsMatchesTests.swift` 4 个用例，用 fixture 目录
  （300 个 `readme-*` + 50 个 `readmx-*`），断言 footer 文字是
  `300 matches · showing 200` 和 `350 matches · showing 200`
- `PanelTypingSearchTests` 原来写死旧格式，改成断言新格式。它在真实 home 上
  跑出 `47,067 matches · showing 200` —— 旧 footer 对这个查询说的是
  "200 of 258277 files"

Rust 54 单元 + 38 集成全通过。Swift 64 个用例 1 个失败：
`PanelFocusTypingTests.test_the_panel_takes_the_focus_without_being_told_to`。
**这个失败是既有的，不是这步引入的** —— 把本步新增的测试类整个排除后它照样
失败（60 个用例 1 个失败），单独跑又通过。是多个 panel 测试类在同一进程里抢
NSApp 焦点的顺序依赖问题，没修。

**两件查到但没动的事：** app 的 NSLog 三小时内一行都没进统一日志（任何级别
都没有），所以日志现在不能当诊断手段；以及内存增长（启动 354MB，实测稳态
应为 60.8MB）。

---

## 第 16 步：仔细对比 fzf，然后砍掉外壳的延迟

起因是"现在的版本完全不行，可以推倒重来"。先量，没重写 —— 量完的结论和重写相反。

### 和 fzf 的全面对比（同一台机器，同一份 258,278 条路径）

**遍历：和 fd 逐条一致。**

```
fd（默认行为 + app 的 8 个 exclude）   258,278 条   0.47s
app 实际索引                          258,276–258,305 条
```

**匹配速度：我们快 2–7 倍。** fzf 的 `--filter` 含约 122ms 读取开销（零匹配的
`zqjvw` 基线），扣掉才是它真正的匹配成本：

| 查询 | 我们（limit 200） | fzf 匹配成本 | 匹配数 |
|---|---|---|---|
| `r` | 16ms | 185ms | 216,230 |
| `re` | 22ms | 163ms | 193,623 |
| `readme` | 21ms | 61ms | 47,071 |
| `src` | 29ms | 152ms | 166,333 |
| `png` | 26ms | 169ms | 193,050 |

**排序：10 个查询里 9 个和 fzf 逐行相同。**

这里先踩了个坑：第一轮对比显示 `path` scheme 下 4/5 不一致，看着像重大
bug —— 实际是 `core/examples/rank.rs` 写死了 `DEFAULT_CRITERIA`，不管传什么
scheme。**库是对的**（`SearchOptions::default()` 用的是
`Scheme::Path.criteria()` = `[Score, Pathname, Length]`，和 fzf
`parseScheme("path")` 一致）。修掉示例后 9/10 逐行相同。

这个示例 bug 值得单独记：**它是用来手工比对 fzf 的工具，而它一直在产生错误的
顺序。** 另外 `fzf_parity` 那 3 个测试是 `#[ignore]`（需要装 fzf），手动跑过，
全通过。

### 唯一的不一致是 fzf 自己的 bug

剩下那个 `config` 的差异全落在含中文的路径上。读 `~/tmp/fzf-src` 的
`result.go`，`byPathname` 注释写着 *"Rune index, to be comparable with
minBegin"*，行为却不是。先下预测再验证：

```
前缀字节数：ab=2  ж=2  代=3        查询 configs，--tiebreak=pathname
预测：ж 第一（val=0）、ab 第二（val=1）、代 最后（MaxUint16）
fzf 实际：ж/configs → ab/configs → 代/configs   ✅ 命中
```

`lastDelim` 是**字节**索引，`minBegin` 是**字符**索引。任何最后一个 `/` 之前
含非 ASCII 的路径都会被 fzf 踢到最后。**我们是对的，fzf 是错的**，没有跟着
复刻。

### 砍掉的延迟

端到端每键 **43ms → 26ms**，外壳开销 **18ms → 2ms**（引擎本身 25ms）：

1. **`scheduleSearch` 里的 12ms `Task.sleep`** —— 它想买的是"手快的人把上一次
   搜索顶掉"，但进入这个 task 本身就是那个时机点，generation 能判断。所以改成
   在 task 开头查一次 generation：同样的合并效果，不再向每一次没被顶掉的击键
   收 12ms
2. **`publish` 里的 `if hits != results`** —— 两百个 hit、每个四个 String 的深
   比较，每键一次，在 main actor 上。改成由调用方传 `resetSelection`：
   scheduleSearch 传 true，refreshResults 传 false。顺带修掉一处和自身注释矛盾
   的行为 —— 注释说 watcher 刷新时保留选中项，代码却只要结果变了就重置
3. **`queryError` 每键无条件赋值** —— `@Published` 不管值变没变都会发通知，所以
   一个完全正常的查询每敲一键都白白触发一次 SwiftUI pass。改成只在变化时赋值

**量化方式**：`KeystrokeBudgetTests` 不卡死一个毫秒数（那测的是机器），而是同一
次运行里同时测击键延迟和 `engine.search`，断言**两者之差 < 8ms**。这样和机器
快慢无关。验证过这个守卫真的有用：把 12ms sleep 加回去，差值变 16ms，测试失败；
拿掉，2ms，连跑两次都过；在满载的全量套件里也过。

（"和机器快慢无关"这句话当时只说对了一半：差值本身还是个绝对毫秒数，于是这条测试
后来一直在 8ms 上下飘。第 32 步把它改成了引擎时间的比例。）

Rust 54 单元 + 38 集成全通过。Swift 65 个用例 1 个失败，是既有的
`PanelFocusTypingTests` 焦点顺序依赖（第 15 步已验证与改动无关）。

### 安装方式的坑

`cp -R build/Seek.app /Applications/` 在目标已存在时是**合并**而不是替换，
结果 bundle 被内核以 `SIGKILL (Code Signature Invalid)` / "Taskgated Invalid
Signature" 杀掉 —— 而 `codesign --verify` 说签名有效，所以很容易误判成代码坏了。
正确做法是先删再拷：

```bash
killall Seek; rm -rf /Applications/Seek.app
cp -R build/Seek.app /Applications/ && open /Applications/Seek.app
```

之前那次 `-600` 大概也是同一个原因。

### 还剩的差距（都不是速度）

- **不流式** —— fzf 边搜边出结果、计数器实时跳；我们等完整答案才一次换掉整个
  列表。现在外壳只剩 2ms，这是下一个能换来体感的改动
- **200 行上限** —— fzf 让你滚完全部匹配
- **没有增量收窄** —— 查询变长时 fzf 可以只在上一轮结果里搜

`core/examples/vsfzf.rs` 是这次的对比基准，可复现。

---

## 第 17 步：放开 200 行上限（按页取）

### 先量，发现两件事

**一、路径缓冲区一直在静默丢结果。** `sk_engine_resolve` 的缓冲区满了不报错，
它 `break` 然后返回"成功解析了几条"，Swift 只遍历回来的那些。固定 256KB 在
只要两百行时够用，一旦要更多就从约 1300 行之后全部消失：

```
改前：limit=2000  → 实际 1279 行    limit=300000 → 实际 1279 行（声称 47067 匹配）
改后：limit=2000  → 2000 行          limit=300000 → 47067 行
```

200 的上限掩盖了这个 bug 整整十六步。

**二、修完缓冲区，真实成本才露出来**，直接放开是不可行的：

| limit | readme | src |
|---|---|---|
| 200 | 26ms | 33ms |
| 1000 | 52ms | 49ms |
| 5000 | 198ms | 175ms |
| 全部 | **2038ms** | **5544ms** |

全量要 2–5.5 秒。之前量到的"354–571ms"是假的 —— 那时只解析了 1873 行。fzf 全量
排序是 60–185ms，它便宜是因为它手里已经有字符串，不需要为每一行解析路径和高亮。

### 所以：击键一页，滚到底续取

- **击键固定要一页（200 行，26–35ms）** —— 延迟不回升，第 16 步的预算测试守着
- **滚到倒数 20 行**（`onAppear`）或**选中项走到倒数 20 行**（`moveSelection`）
  触发 `loadMoreResults()`，页大小翻倍，按人的速度付钱
- **`End` 键**（`selectLast`）跳到已取回的末行并请求下一页，不再假装那是最后一个
  匹配
- **新查询重置回第一页**，否则滚动过一次之后每一次击键都要为没人要的行付钱
- footer 从 `showing 200` 改成 `first 200` —— 前者是上限，后者是位置

**能这么做的前提是排序是全序**（score → pathname → length → 输入位置），所以
"最好的 N 条"唯一，长列表必然以短列表开头，用户正在看的行不会在脚下重排。
`test_a_longer_page_begins_with_the_shorter_one` 就是守这个的。

### 顺带去掉的共享可变状态

limit 以前是引擎状态（`sk_engine_set_limit`）。那样一来"滚动要更多行"会改变
**同时在飞的每一个**搜索返回的条数 —— watcher 的刷新和击键搜索都读同一个值。
现在 limit 是每次调用的参数（复用已有的 `capacity`），`sk_engine_set_limit` 删掉。

### 测试

`ResultPagingTests` 6 个用例：长列表以短列表开头、无重复、滚到底真的会续取到
全部 900 条、新查询回到第一页、取回新页不动选中项，以及 `test_long_paths_do_not
_silently_vanish`。

最后那个我验证过它真的有用：把缓冲区改回固定 256KB，它报 **512 of 900 results
were dropped**。第一版里我还把断言写错了 —— 用返回结果的字节数判断"fixture 够不
够大"，而结果本身是被截断的，于是它正好因为该失败的原因而通过。改成从 fixture
本身算。

Rust 54 单元 + 38 集成全通过。Swift 71 个用例 1 个失败，仍是既有的
`PanelFocusTypingTests` 焦点顺序依赖。

---

## 第 18 步：空转的 CPU 和不断变大的内存

**起因**：用户第三次报同一个现象 —— 输入 `human` 有结果，删空之后再输入任何字母
都不更新。前两次我都没找到，这次换了找法。

### 先否掉自己的三个猜测

写了三个探针，全部用来**排除**，没有一个命中：

| 猜测 | 探针 | 结果 |
|------|------|------|
| 分页失控（行的 `onAppear` 被布局触发） | 真实面板里静坐 20 秒 | 行数稳在 200，不涨 |
| 模型层卡住 | 直接写 `model.query` | `a`/`b`/`c` 分别 62/80/70ms 更新 |
| 字段丢焦点 / 监视器吞键 | 合成真实按键事件打进事件队列 | `human` → 删空 → `rea` 全程正常 |

最后那个探针第一版是坏的：`postEvent` 在 XCTest 里没人抽队列，所以**连 `human`
都没进去**，看起来"复现了"。手动 `nextEvent` + `sendEvent` 抽干队列之后，全通。
一个不会失败的测试比没有测试更坏。

### 真正的根因：在跑着的那个进程里

干净实例上复现不出来，就去量那个**真正卡着的进程**：

```
PID 96995  RSS 365712KB  %CPU 34.8  运行 9:51
```

一个待机的启动器，平均烧掉三分之一个核。`sample` 采样：主线程空闲（阻塞在
`mach_msg`），CPU 全在

```
Engine::publish → Corpus::build → Index::relative_path_with_root
```

对应代码：`sk_engine_apply_changes` 里只要有一条变更就调 `eng.publish()`，而
`publish` 的全部内容就是 `Corpus::build(index)` —— 把全部 258,286 条路径重新算一
遍、每条新分配一个 `String`。watcher 每 400ms 报一批，开发机上永远有东西在变。

这一条同时解释了用户说的三件事：常驻烧 CPU（面板抢不到时间，打字看着没反应）、
内存一路爬（每 400ms 重新分配再丢弃整份路径集）、以及卡住。

### 顺手挖出来的第二个根因

写 fixture 的时候发现索引里有**两个 `root`**。`Index::ensure_dir` 从
`current = None` 开始，第一个组件的查找是 `current.and_then(...)` —— 恒为 `None`，
所以永远匹配不上任何已有条目。`find_dir_by_path` 是同一个错的镜像
（`let parent = current?;` 在第一个组件直接 return）。

一个根因，三个后果：

- `find_by_path` 永远返回 `None` ⇒ **`Index::remove` 从来没删掉过任何东西**，
  删掉的文件永远留在索引里
- 每次 watcher upsert 都新建一整条祖先目录链 ⇒ 索引无界增长
- 同一个文件在面板里出现两次（一次扫描的相对路径，一次 watcher 造的
  `Users/bioguo/...`）

两个函数都没有锚定到索引自己的 roots。新增 `resolve_root`：把路径解析到它所属的
根，再走剩下的组件。顺带 `sk_engine_set_roots` 现在做 `canonicalize` —— macOS 上
`/var` 是 `/private/var` 的符号链接，FSEvents 报的是后者，不规范化就永远对不上。

### 改了什么

**1. 语料库增量更新**

- 删掉 `Corpus::names` 和 `CorpusEntry::name`。前者 258k 个 `String`**写进去之后
  从来没人读**，后者只是用来填前者。每条路径的分配从 3 次降到 1 次
- `paths: Vec<String>` → `Vec<Arc<str>>`
- 新增 `Corpus::updated(&self, index, changed)`：把旧快照和变更 id 归并走一遍，
  没变的路径按 `Arc` 搬过去，只重建变了的那几条
- `Index` 自己记 `dirty` 列表（`#[serde(skip)]`，不进快照）。记在索引里而不是让
  调用方收集，是因为 `upsert` 会顺手创建缺失的父目录 —— 那些新行调用方根本不知道

测到的效果（258,316 条真实索引）：

| | 耗时 |
|---|---|
| `Corpus::build`（全量重建） | 53.5ms |
| `Corpus::updated`（一批 5 个变更） | 1.18ms |
| | **便宜 45 倍** |

53.5ms 还是**改进后**的全量重建（已经少了两份字符串拷贝）。旧版更贵。

**2. watcher 遵守排除列表**

`exclude_patterns` 以前只在扫描时生效，apply 路径完全不过滤。新增
`scan::ChangeFilter`，每批编译一次，逐级测试祖先 —— 因为 gitignore 语义里
`Library` 只匹配 `Library` 本身，walker 是靠**不往里走**来排除子树的，所以它从不
匹配 `Library/Caches/junk.bin`。逐级测祖先才等价于剪枝。

同时套用 hidden 规则（`.git`、`.DS_Store`），以及"不在任何根下面就不要"。

**没覆盖**：`.gitignore`/`.fdignore`。扫描遵守它们，apply 路径不遵守 —— 一个只被
gitignore 的目录（比如 `target/`）仍然能从 watcher 进索引。要补得把整套 ignore
匹配栈搬到 apply 路径上。

**3. 面板收起时不刷新**

`refreshResults()` 以前每个 watcher tick 都跑一次完整搜索 + 路径解析 + 推给
SwiftUI，哪怕面板是收起来的、没人在看。加 `AppModel.isPanelVisible`，由
delegate 在 `showPanel`/`hidePanel` 里置位。面板打开时本来就会清空查询，所以没有
"积压的列表要追上"这回事。

### 测试

- `core/tests/incremental_corpus.rs`（8 个）—— 唯一要紧的不变量：增量更新后的快照
  必须和全量重建**逐字段相等**。覆盖新建、修改、删除、删目录带走子树、新目录链、
  混合批次、空批次。还有一个断言 `Arc::ptr_eq`：没变的路径必须是**同一块分配**，
  否则这次改动等于没做
- `core/tests/watcher_exclusions.rs`（5 个）—— 走 C ABI，手工 apply 而不等真实
  watcher，所以是确定性的
- `app/Tests/SeekTests/IdleRefreshTests.swift`（3 个）—— 能区分"刷新有没有
  发生"：把列表手工清空，查询还在，看它会不会被填回来
- `app/Tests/SeekTests/PanelRetypingTests.swift`（1 个）—— 用户报的那个序列，
  合成真实按键走完整路径（含本地事件监视器）。它**第一次跑就通过** —— 这正是把
  搜索方向从打字路径挪到后台工作的那一步。留着，免得以后坏了没人知道

### 修掉一个假测试

`PanelDrawsResultsTests.test_the_rows_of_a_real_search_are_drawn` 开始失败。看
渲染出来的 PNG：面板画得完全正确，14 行、高亮、footer 写着
`47,067 matches · first 200`。

问题在断言：它把"行有没有画出来"折成了"墨迹是否超过空态 3 倍"，而墨迹多少取决于
磁盘上**恰好**有哪些路径、路径多长 —— 这次命中的全是 `README.md` 配极短的父目录名
（`eli5`、`CellHet`），墨迹就少了 4%。另外 `listBand` 用 point 坐标去索引 Retina
位图（2 倍），只扫到了图像上半部分。

改成数**画了多少行文字**（有墨的扫描线连续段），并按位图比例换算坐标。结果行 16
段、空态 4 段，断言 `rows > emptyRows * 3`。这个量法不受路径长度影响，也不会被一
整块色块骗过去。

### 结果

装好之后实测同一台机器、同样待机：

| | 旧（PID 96995，运行 9:51） | 新（PID 5654，运行 41:40） |
|---|---|---|
| CPU（生命周期平均） | **34.8%** | **4.0%** |
| CPU（瞬时） | — | 3.3–4.9% |
| 内存 RSS | **365 MB，持续增长** | **211 MB，平的** |

内存那条是最硬的：9:33 时 211,280KB，41:40 时 210,960KB —— 半小时里没有涨。旧版
是 354MB 启动、几小时后 586MB。

### 剩下的 4%：不是这次改出来的

新版待机仍占 3.3–4.9%，5 个线程在持续做系统调用（20 秒里每个涨约 0.12s 系统时间、
0.02s 用户时间）。采样看过去：`seek-scan` 线程在启动 41 分钟后仍卡在

```
ignore::WalkParallel::visit → Thread::join → __ulock_wait
```

**旧二进制在 9:51 时是完全一样的状态**（同样的栈，同样 6 个 `read_dir` 帧），所以
这是既有问题，不是第 18 步引入的。旧版的 34.8% 里，这 4% 一直在，只是被语料库重建
的 30% 盖住了。下一步应该查这个：扫描结束之后，walker 线程为什么还没收掉。

### 测试结果

Rust 105 个测试全过（含 `fzf_parity` / `fd_parity` 的 7 个 ignored 用例，排序和
遍历一致性都还在）。Swift 75 个用例 1 个失败，仍是既有的 `PanelFocusTypingTests`
焦点顺序依赖（单独跑通过）。

### 一个诚实的保留

用户报的那个**确切**现象 —— 删空之后输入任何字母都不更新 —— 我在干净实例上始终
没能复现。定位到的是那个让面板持续抢不到 CPU 的真实缺陷，两者高度一致（34.8% 的
常驻占用足以让打字看起来没反应），但严格说，我证明的是"进程在空转烧 CPU"，不是
"按键路径有逻辑错"。需要在真机上再走一遍那个序列确认。

### 还剩

- **不流式** —— fzf 边搜边出结果、计数器实时跳；我们等完整答案才换列表
- **没有增量收窄** —— 查询变长时可以只在上一轮结果里搜
- **滚得很远会慢** —— 翻倍到 20000 行要 669–866ms。真要做到 fzf 那样"滚动免费"，
  得改成只为可见行解析路径和高亮（惰性 hydration），那时每次击键仍要付全量排序的
  113–243ms —— 也就是 fzf 自己的延迟。这是个取舍，不是纯改进

---

### 还没做完

- ⬜ **UI 精修（继续）** —— 第 12 步修掉了截图里的四个缺陷，面板现在跟着内容
  变高。还可以做的：空查询时显示最近打开过的文件（需要存访问记录，是新功能
  不是精修）；结果行的图标目前只分文件/文件夹，没有按类型区分
- ⬜ **性能优化** —— 预算已经达成，继续优化的空间还有：窗口是 `Vec<char>`
  （ASCII 下每字符 4 字节），fzf 的 2 字符融合快路径（`fuzzyMatchV2Two`）也没移植
- ⬜ `content:` 操作符（fzf 的内容搜索）—— 设置里显示为禁用；查询里输入它会被
  明确拒绝，而不是静默匹配全部
- ⬜ 首启动的辅助功能权限请求（README 提到，当前用 Carbon 热键规避了）

---

## 第 19 步：一个不响应的目录能把整个索引拖死

**起因**：第 18 步把 CPU 从 34.8% 压到 4.0% 之后，用户要求查剩下的 4%；同时发来
一张截图 —— 查询框里是 `ls`，结果行却还高亮在上一次的 `nat` 上，搜索框里转着圈，
footer 写着 "Indexing…"。

### 证据链

全部量在那个正在跑的进程上，没有一步是推测：

| 事实 | 怎么拿到的 |
|------|-----------|
| 5 个 walker 线程整段采样（3256/3256 样本）停在 `open$NOCANCEL` | `sample` 按线程分栈 |
| 另外 5 个 walker 在 `ignore` 的 1ms 退出握手里空转 | 栈顶是 `WalkParallel::visit` → `thread::sleep` |
| 主扫描线程卡在 `Thread::join` | 同上 |
| rayon 的 10 个 worker 在 `wait_until_cold`，正常停驻 | 排除线程池饱和 |
| `sk_engine_search` 只取 corpus 锁，不碰 `inner` | 读代码，排除搜索被扫描阻塞 |
| 走盘 54 分钟后自己返回，快照才写出 | 进程 18:30:27 启动，`index.skidx` mtime 19:24:04 |
| 同配置的 `scan_blocking` 从终端跑 0.72 秒 / 258,327 条 | 排除"走盘天生慢" |

链条是：`opendir()` 被内核堵住 → 走盘不返回 → `sk_engine_finish_scan` 卡在
`scanner.join()` → 索引永不发布 → `isIndexing` 永远为真、footer 永远 "Indexing…"，
而其余 worker 每 1ms 醒一次查退出条件 —— 那就是那 4%。

### 两个诚实的保留

**一、没能指认出具体是哪个目录。** 机器上当时唯一挂住的两个路径，一个是 `/Volumes`
下死掉的 NFS 挂载（在扫描根之外），一个是 `~/Library/Mobile Documents`（被 `Library`
剪掉）—— 按理两个都到不了，所以还有第三条没点出来的路径。TCC 日志是空的，快照格式
也不暴露明文名字，"被权限网关挡住"和"跨卷挂载"这两种可能没能分开。修复不依赖这个
答案：A 挡住跨卷，B 兜住其余一切。

**二、协作线程池饱和的猜测是错的。** 我本以为 Swift 并发池被占满、搜索排不上队，
采样显示那 10 个阻塞线程是 rayon 的空闲 worker，池子是空的。截图里"行还高亮在旧
查询上"的具体机制，读完 `scheduleSearch` / `publish` / generation 计数器之后仍然
没能解释 —— generation 逻辑是对的，`publish` 从不拒绝空列表。那是独立的一个问题，
要在索引能正常完成的构建上重新复现。

### 修复 A：走盘不跨文件系统

`ScanConfig.same_file_system`，默认开，`run_scan` 里 `builder.same_file_system(true)`。
走盘只留在每个 root 所在的那个卷上：`/Volumes` 下的 NFS、磁盘镜像、移动硬盘，
`~/Library/CloudStorage` 的 file provider 卷，全都在边界停住。`fd` 把这个写成
`--one-file-system`；对一个整天跑着的东西，它该是默认值。

没有给 UI 开关 —— 用户明确说临时挂载不要扫，一个没人会去翻的偏好是多余的配置。
Rust 侧 `same_file_system: false` 的路径仍然存在，并且有测试守着。

测试（`core/tests/one_file_system.rs`）用 `hdiutil` 真的在临时 root 里挂一个 8MB
镜像，因为这个 bug 讲的就是文件系统边界，短于此的东西测不出它；`hdiutil` 不需要
权限。第二个测试显式用 `same_file_system: false` 断言那个文件**能**被找到 —— 没有
它，第一个测试在 fixture 坏掉或镜像为空时也会绿。

### 修复 B：扫描带 deadline

`ScanConfig.timeout`，默认 60 秒。`Scanner::is_finished()` 变成
`walk_finished() || timed_out()`，`join()` 换成 `finish()`：只在走盘已经返回时 join，
否则置停止位并 detach —— 堵在 `opendir()` 里的线程没法取消，而 join 它正是把索引
拖死 54 分钟的那一步。

超时的后果是**索引不完整**，所以 `sk_engine_scan_complete` 把这件事告诉外壳，
footer 写 "N files indexed · scan cut short"。把残缺的索引报成整块磁盘，和永远挂着
"Indexing…" 是同一种谎，只是声音小一点。

### 量出来的结果

| | 修之前（卡住的实例） | 修之后 |
|---|---|---|
| 索引发布 | 启动后 54 分钟，或永不 | 启动后约 10 秒 |
| 稳态 CPU | 4.0%（生命周期均值） | 0.0–0.4% |
| 线程数 | 19 | 9 |
| 快照 | 2,860,022 字节 | 2,844,567 字节（−0.5%，就是挂载卷那点条目） |

**一个数字要更正**：第 18 步报的"211MB 平稳"是在扫描永远没完成的实例上量的 ——
它的索引从没重建过，只有恢复的快照。完整索引的真实稳态是 **285MB**，258k 条约
1.1KB/条。这偏高，是下一步该看的东西。

测试：Rust 111 个通过（新增 6 个），8 个 parity 测试（`fd_parity` / `fzf_parity` /
`app_path_parity`）全过 —— 遍历和排序的契约没破。Swift 78 个，1 个
`PanelFocusTypingTests` 失败而单独跑通过，是既有的顺序依赖。

**留下的坑**：超时之后索引就一直残缺到重启，没有自动重扫。A 到位之后 B 本该永远
不触发，所以没为它加重试机制 —— footer 会说，用户可以自己重启。

---

## 第 20 步：285MB 的内存去哪了

**起因**：第 19 步末尾更正了一个数字——第 18 步报的"211MB 平稳"是在扫描永远没完成
的实例上量的，它的索引从没重建过。完整索引的真实稳态是 285MB。

### 先量，别猜

写了 `core/examples/mem.rs`，按阶段报 RSS。第一轮数字就推翻了我的预期：

```
     5.7 MB  baseline
    66.8 MB  恢复快照之后（面板打开时就是这个）
   117.9 MB  走盘结束
   149.1 MB  finish_scan 之后（条目数正确，没有重复）
   149.2 MB  把整个 engine 释放之后   ← 一个字节都没还给系统
```

活数据只有 40.6MB（index 13.5 + corpus 27.1）。所以**问题不是数据结构胖，是索引那
一瞬间的峰值**，而峰值过后 `malloc_zone_pressure_relief` 返回 0、`sk_engine_free`
之后 RSS 一动不动。

三轮循环区分了泄漏和高水位：114 → 148 → 156MB，增量 +34 然后 +7，**收敛**。内存是在
复用的，只是峰值由大约 60 万个短命小分配组成（每个扫描条目一个 `PathBuf` 加一个
`String`），散落在 malloc 的小对象区域里，每个区域都还留着活对象，所以整页还不回去。

**探针自己的两个 bug**，值得记：`--launch` 第一版我把 excludes 用换行拼，而 FFI 是按
逗号切的，结果 `Library`、`node_modules`、`.git` 全被走了，跑出 560,880 条和 224MB，
看起来像是 app 在重复索引。核对分隔符之后才干净。另一版的 sort 用的是 `sort_by_key`
而真实代码已经换成 `sort_by_cached_key`，时间数字对不上。探针和被测代码不一致，量出来
的东西就是假的。

### 修峰值：三处

| 浪费 | 实测 |
|------|------|
| `Scanner::next_entry()` 是 `buffer[cursor].clone()`，原件留在 buffer 里 → drain 完两份完整条目同时在 | 59.1 → 103.5MB；条目按字节数共 47.2MB，对得上 |
| `rebuild_index` 迭代 `&sorted`，207k 条活到索引建完 | 丢掉它们时 138.0 → 109.8MB |
| `rebuild_index` 最后才 `self.index = index`，新旧两个索引同时活着 | +13.5MB |

`next_entry` 换成 `drain_into(&mut Vec)`：`std::mem::take` 把整个 buffer 搬走，调用方
的 Vec 为空时**连拷贝都没有**——buffer 的那块分配直接变成调用方的。顺带把 drain 从
23.8ms 变成 **375ns**。

`rebuild_index` 改成 `into_iter()` 边消费边插入，每条的两块堆内存插完即释放；目录的
路径直接 move 进 `dir_ids` 而不是 clone。另外 `sort_by_key(|e| e.path.components()
.count())` 每次比较都重新数一遍路径分量，实测 200ms，换 `sort_by_cached_key`。

### 修活数据和碎片：Corpus 换成 arena

`Corpus.paths` 原本是 `Vec<Arc<str>>`——20 万个独立分配，既是 27.1MB 活数据，也是碎片
的根源。换成两代 arena：

```rust
base: Arc<String>,   // 和派生它的那个快照共享，不可变
overflow: String,    // 本快照为改动条目追加的
spans: Vec<Span>,    // {start: u32, len: u32}，offset 落在 base ++ overflow 的虚拟拼接上
```

arena 用 `String` 而不是 `Vec<u8>`：切片时的 UTF-8 边界检查是 O(1)，换成字节数组就得
在每次击键时验证全部路径，或者用 `from_utf8_unchecked`。

**和第 18 步的冲突**是这一步的关键。那一步靠 `Arc<str>` 共享让增量更新只是指针拷贝
（1.18ms）。两代 arena 保住了它：`updated` 把 `base` 做一次 Arc 指针拷贝，未改动的条目
只搬 8 字节的 span，改动的追加进 `overflow`。`tests/incremental_corpus.rs` 里原来的
`Arc::ptr_eq` 断言换成了更直接的一条——同一条未变路径在两个快照里 `as_ptr()` 完全相同，
而且这个性质从外部就能观测，不需要碰私有字段。

`overflow` 会随每个 watcher 批次增长而没人取出，**那就是第 18 步索引无界增长的同类**。
所以涨到 base 的四分之一加 1MB 下限就整体重建、两个 arena 并回一个。
`the_overflow_is_compacted_rather_than_growing_for_ever` 守着它——摘掉压实分支，900 字节
的 base 下 overflow 涨到 3.6MB，测试变红。

`Corpus::build` 的 arena 预留了容量。让 `String` 靠倍增长到 17.6MB，realloc 时新旧两块
同时在，把这一步刚省下的 8MB 又加回了峰值。

### 量出来的结果

| | 原始 | 修峰值三处 | 再加 arena |
|---|---|---|---|
| 分阶段稳态 | 136.5 MB | 110.8 MB | **89.9 MB** |
| app 启动序列峰值 | 151.5 MB | 107.5 MB | **110.9 MB** |
| 三轮循环收敛 | 155.7 MB | — | **110.6 MB** |
| corpus 活数据 | 27.1 MB | 27.1 MB | **22.3 MB** |
| drain | 23.8 ms | 375 ns | 583 ns |
| corpus 构建 | 53.5 ms | — | 42–49 ms |

**真实 app：285MB → 250MB。** 降幅比引擎自己的小，因为 250MB 里引擎只占约 110MB，
剩下约 140MB 是 AppKit/SwiftUI 的底子，从 Rust 侧动不了。

**搜索没有回归**，这是这一步最重要的闸门。同一台机器、同一个 bench，对比第 11 步记录的
数字：`src` 29.3 → 21.0ms，`test` 43.4 → 31.7ms，`cargo` 17.0 → 9.5ms，
`index` 12.6 → 11.1ms。不过语料从 258k 变成 208k（家目录本身变了），按比例折算后是
"持平到略好"，不夸大成提速。8 个 parity 测试（`fd_parity` / `fzf_parity` /
`app_path_parity`）全过，排序和遍历的契约没动。

Rust 115 个测试通过（新增 4 个）。Swift 78 个里 3 个失败，**单独跑全部通过**——是测试
顺序污染，不是回归。污染从 1 个变成 3 个，因为新加的测试改变了执行顺序；其中
`PanelRetypingTests` 的失败形态是"上一个测试在查询框里留下了 `o hua`"，和用户截图里那个
陈旧查询是同一个形状，追那个 bug 时值得从这里开始。

---

## 第 21 步：增量收窄（以及它为什么救不了慢查询）

**起因**：fzf 快的一条关键机制。新查询严格扩展旧查询时（`na` → `nat`），它把上一轮
的结果集当候选池，不碰全量语料，所以越打越快。我们每个键都对 208k 条重跑一遍。

### 单调性边界：三个真实的反例

要复用上一轮的池，必须保证 `matches(Q2) ⊆ matches(Q1)`。纯模糊查询是单调的——"nat"
是子序列则 "na" 必然是——但这个查询语言比 fzf 的默认模式丰富，有几种"延长查询"
反而**放大**结果集：

| 从 | 到 | 为什么不能复用 |
|---|---|---|
| `!na` | `!nat` | 排除变少，结果集变大 |
| `a$` | `a$x` | 后缀项变成要求字面 `$` 的模糊项，两个集合互不包含 |
| `re` | `re:a` | 模糊 `r`,`e` 变成正则 `a` |
| `sr` | `sr*` | 模糊项变成 glob |

所以 `query::narrows()` 要求三条，合起来是充分的：Q1 没有 negative 项；Q1 的所有
positive 项都是模糊项；Q2 的前 n 项也是模糊项且逐位以 Q1 对应项为前缀（多出来的项
任意种类）。这个关系是**自反**的，所以面板翻页时用同一查询换更大 limit 也直接命中池。

失效靠两个版本号：corpus 版本（池里是快照内的位置偏移，`Corpus::updated` 会移动它们）
和 options 版本（匹配结果是一组规则下的事实）。两者必须**一次读出** —— 分两次读的话，
中间来一次 publish 就会拿到一个指向别的语料的 stamp。

### 测试：差分 + 真值 + 变异

单元差分测试（`incremental_narrowing.rs`）在同一语料上把收窄结果和冷扫逐字段比对：
id、score、匹配区间、高亮位置、顺序、匹配数。并断言 `scanned` 真的变小了——不然
池被接受后悄悄忽略，测试照样全过。

FFI 层（`narrowing_cache.rs`）测的是缓存本身，断言的是**真值**而不是另一次运行：
池取好之后到达的文件必须出现在结果里，放宽大小写后 `TEMPL` 必须匹配 100 条，
退格后匹配数必须从 20 回到 100。

**我自己写测试踩的两个坑，值得记**：第一版跨两个引擎比 id——而 id 是扫描时分配的，
同一棵树的两次并行走盘顺序不同。改成比路径后还是不等，因为 fzf 的末位 tiebreak 就是
输入顺序，所以**两个引擎连顺序都不该相等**。最终改成在同一引擎上比（第一次搜索天然
是冷的，这个参照物不依赖被测的规则）。第二个坑：`apply_changes` 的路径没
canonicalize，而 `set_roots` 会，macOS 上 `/var/folders` → `/private/var/folders`，
于是变更被 ChangeFilter 当成"在所有 root 之外"丢掉——真实 watcher 从 FSEvents 拿到的
是已解析路径，所以产品没问题，是测试在说谎。

那 5 个 FFI 测试在缓存实现之前就**空过**了（它们守的是"必须拒绝"），所以逐条拆守卫
做了变异检查：去掉 `narrows()` → 退格和取反两个测试红；stamp 不比 corpus 版本 →
文件到达测试红；不比 options 版本 → 大小写测试红。每条守卫都是承重的。

### 不值得留的池

第一轮实测暴露了一个问题：`car` 的池有 158,635 条（模糊子序列太宽松），复用只省
0.2ms，而构建它要分配并拷贝最多 832KB。所以**匹配数超过语料一半时不建池**，下一键
回去扫全量——那本来就是这么大的池的等价物。门槛之后前几键的成本和改动前一致。

### 实测：收益的形状

同机 208,156 条语料，逐键，10 次平均：

```
typing "seek"              typing "readme"            typing "test"
keys  池      冷      收窄      keys  池      冷     收窄   keys  池   冷      收窄
f     —     8.40   8.44        r     —    11.18  11.53    t    —  16.59  17.26
fu    —     6.58   8.23        re    —    12.94  12.86    te   —  22.06  22.03
fuz   53742  1.98   1.24       rea   —    12.54  13.70    tes  —  23.96  24.71
fuzz  2856   1.79   0.62       read  103029 8.30  7.78    test —  27.75  28.11
fuzzy 445    1.73   0.78       readm 49018  5.63  4.50
fuzzyl 314   1.56   0.32       readme 17819 5.60  4.09
```

`seek` 第 6 键起 1.5ms → 0.3ms（5 倍）。`readme` 从第 4 键起省 20–27%。
**`test` 和 `src` 一键都没有收窄** —— 它们每一键都匹配超过一半语料，所以永远不建池。

### 为什么救不了慢查询：一个模型

把 limit 从 500 降到 1，时间几乎不动（`readme` 4.56 → 4.54ms，`test` 27.20 →
27.01ms），所以瓶颈不是高亮位置那一遍。把三个数拼起来：

```
时间 ≈ 1.4ms（扫过 208k 条的固定成本）+ 约 200ns × 命中数
```

`seek` 几乎全部拒绝，208k 条只要 1.39ms（每条 6.7ns）；`test` 命中 167,196 条，
27ms 里约 25.6ms 是这些**命中项**的 DP 打分。

**收窄缩小的是候选池，而命中项全都在池里。**所以它能削掉的只有那 1.4ms 的基础成本，
对 `test` 上限就是 5%。这不是实现没做好，是这个办法的天花板。

### 诚实的结论

功能是对的、有差分测试和变异检查守着、不该生效时成本为零、该生效时 3–5 倍。但：

- 它把选择性查询的 Rust 侧从 1.5ms 压到 0.3ms，而外壳还有 2ms 固定开销，所以
  **人感觉不到这个改善**。
- 真正发涩的那一类（`test` 27ms、`src` 20ms）**一键都没生效**，而且结构上也不可能
  生效。

要动那 27ms，只有两条路：让单条命中项的 DP 更便宜（现在 153–255ns，4–6 字符模式
对约 85 字符路径），或者不给进不了前 500 名的命中项算完整排序键。两条都是独立的
工作，不是这一步的延伸。

内存代价：一条池上限为语料一半 × 4 字节 = 416KB，不 pin 任何别的东西。

## 第 22 步：慢查询的真正杠杆

第 21 步留下的结论是"要动 `test` 的 27ms 只有两条路"。两条都做了，`test` 从 30.4ms
降到 11.0ms。但先要订正第 21 步写错的一个数字。

### 订正：970ns，不是 200ns

第 21 步写的模型是 `1.4ms + 约 200ns × 命中数`。那个 200ns 是 rayon 分到 8 核之后的
**等效**值，不是单条命中的真实成本。单线程量下来：

| 模式 | 命中数 | ns/命中 | ns/未命中 |
|------|--------|---------|-----------|
| te | 196,677 | 534 | 33 |
| tes | 181,932 | 713 | 40 |
| test | 167,196 | **970** | 47 |
| testi | 136,133 | 1092 | 57 |
| testin | 114,430 | 1301 | 63 |
| testing | 33,393 | 1443 | 74 |

每多一个模式字符 +218ns，截距 316ns。所以 `test`（4 字符 ⇒ 3 行 DP）的 970ns 里
**654ns 是 DP 的那几行**，316ns 是相位 1、窗口物化、相位 2 和回溯。窗口宽约 70，
折算 **3.1ns/格** —— 一格只做几次 i16 加法和比较，这个数字高了两三倍。

### 一个没预料到的实测：截断线就是满分线

每个查询的第 500 名得分、最高分、中位数，和理论上限 `m×16 + 2×maxB + (m−1)×maxB`：

| 查询 | 命中 | 第 500 名 | 最高 | 中位数 | 理论上限 |
|------|------|-----------|------|--------|----------|
| test | 167,196 | **109** | 109 | 64 | 109 |
| src | 119,010 | **84** | 84 | 47 | 84 |
| index | 37,032 | **134** | 134 | 50 | 134 |
| cargo | 29,421 | 87 | 134 | 50 | 134 |
| readme | 12,284 | 156 | 159 | 63 | 159 |

对 `test`／`src`／`index`，**第 500 名就是满分，而满分正好是理论上限**。`test` 有
37,981 条路径同时拿 109 分，前 500 名是在这些满分项里靠 Pathname／Length 分出来的。
也就是说 167,196 条命中里 **129,215 条（77%）的分数在截断线以下，谁都看不见，
但我们给每一条都跑了完整 DP**。

### 第一步：收紧 DP 内层循环

改前每格做 6 次带边界检查的 `Vec` 下标访问，全部在一个扁平矩阵上算偏移。改成：

- 每行先 `split_at_mut` 切出上一行和当前行，把六个输入输出都切成**长度相同**的
  切片，循环跑 `0..cols` —— 边界检查整体消失。
- **左邻不读内存**：`h[row+local-1]` 就是上一格刚算出的分数，用寄存器带着走。
- **不再清零整个矩阵**：`clear() + resize(width*m, 0)` 每条候选写 `width*m` 个零
  （70×4 的两个 i16 矩阵 ≈ 2.2KB），而 DP 马上要覆写掉大部分。改成只 `resize` 不
  `clear`，每行手工把 DP 不会写、但回溯会读的那一小段前缀清零。

算术一个字没改，所以 8 个 parity 闸门就是它的测试。

#### 意外：代码体积让单字符查询慢了 15%

改完 `s`／`t` 从 12.99／13.36ms 退到 15.73／15.57ms（隔离测量，取 best）。而 m=1 走
`match_single`，**根本不碰 DP**。唯一的解释是代码体积：`run_inner` 变大后，单字符
的路径被挤在一个更大的函数体里。

把相位 2–4 拆成 `score_matrix`，标 `#[inline(never)]`，`run_inner` 回到小体积 ——
回退消失（`s` best 12.3–14.2ms，和改前的 12.99 同一区间），而 DP 查询的收益反而
更大。这一条不是猜的：拆之前和拆之后各量了三遍。

### 第二步：进不了前 N 名的命中项不打分

#### 论证

两条事实叠起来：

1. 任何命中的得分都 **≤ 上限** = `m×SCORE_MATCH + 2×maxB + (m−1)×maxB`，其中
   `maxB` 是该 scheme 的 bonus 矩阵里的最大值（从矩阵里取，不写死）。每个字符值
   `SCORE_MATCH` 加一个 bonus，首字符的 bonus 算两次，而 gap 只会减分。
2. 一条**不含模式连续子串**的路径，在任何对齐下都至少有一个 gap，一个 gap 至少扣
   `SCORE_GAP_START` = 3 分，所以它的得分 **≤ 上限 − 3**。

于是：一旦有 `limit` 条候选的得分落在上限的 2 分之内，第 `limit` 名的分数就不低于
`上限 − 2`，而一条没有连续子串的路径 ≤ `上限 − 3`，**严格低于至少 `limit` 条候选**，
不可能进前 `limit` 名。

被剪掉的候选仍然要**计数**，`FuzzyMatcher::occurs_in` 只跑相位 1（每个模式字符一次
memchr）就能回答"是不是命中"，47ns。所以 `matched` 和增量收窄的候选池一条都不少。

#### 计数器为什么是共享的

一开始想用 per-worker 计数器（无竞争、无原子量）。但 rayon 把 208k 条候选切成远多于
核数的小块，每块里很难凑够 500 条满分候选，剪枝基本不会 arm。所以改成一个共享
`AtomicUsize`：

- 只有得分 ≥ `上限−2` 的候选才 `fetch_add`，而且 arm 之后就停止写入 ——
  那条缓存行之后是只读的，每条候选只付一次 relaxed load。
- **论证不依赖线程顺序**：计数器到了 `limit`，说明全局确实有 `limit` 条候选在那个
  分数之上，无论它们是哪几条、谁先谁后。
- 哪些候选被剪掉**会随调度变化**，但被剪掉的候选是可证明进不了前 N 名的，所以
  **结果不变**。

#### 四条守卫

| 守卫 | 为什么 |
|------|--------|
| 只对**单个** fuzzy term 生效 | 多 term 的得分是各项之和，不再被任何单项的上限约束，论证整条失效 |
| 模式必须是纯 ASCII | 子串检测按字节走，而匹配器默认折叠重音字母（fzf 的默认行为） |
| 路径必须是纯 ASCII | 同上，但是逐条判断：`src/tést/main.rs` 在折叠下含有 `test` 的连续串、拿满分、该排第一，而字节子串检测说它压根不含 `test` |
| 模式长度 ≥ 2 | 单字符时"包含这个字符"和"匹配这个字符"是同一个问题，剪枝只可能白花一次 memchr 去确认一个非命中。实测给 `s` arm 剪枝的代价是 9% |

最后一条是**成本守卫，不是正确性守卫** —— 去掉它结果一模一样，只是更慢，所以
任何检查结果的测试都抓不到它。测试里钉的是让它安全的那条等价性
（`occurs_in` ≡ `literal::contains`），而不是一个空断言。

### 测试

- **属性测试**（`tests/score_bound.rs`，5 条）：在整个夹具语料上断言
  `score ≤ 上限`，以及对每条不含连续子串的 ASCII 路径断言 `score ≤ 上限 − 3`。
  打分常数一旦改了而上限没跟着改，这里立刻红。
  还钉了三条边界：满分是真能达到的（否则剪枝永远不 arm，是需要维护的死代码）、
  上限跟着 scheme 走（default 的边界 bonus 比 path 大）、以及重音路径那个漏洞。
- **差分测试**（`tests/top_rows_pruning.rs`，10 条）：oracle 是 `rank_candidates`
  —— 它给每条候选打分再整体排序，没有任何可剪的东西，而且 tiebreak 链和扫描方向
  用的是搜索自己的。逐字段比 id、score、start、end 和高亮位置，limit 取 1／10／200。
- `scored` 作为可观测量加进 `SearchOutcome`，和第 21 步的 `scanned` 同一个理由：
  一个什么都没剪的剪枝会让结果完全正确、只有时钟能看出问题。
- **变异检查 7 条**，每条都红：去掉 `is_ascii` 守卫、上限余量 2→3、允许多 term、
  计数门槛放低 30 分、未打分候选也参与排序、剪掉的候选不再计数。
  第 7 条（允许单字符）**没红** —— 这正是上面说的成本守卫，已据此改掉了那个空断言。

#### 我自己写测试踩的三个坑

1. `matched` 断言成 4000，实际 4001 —— `src/test` 这个**目录自己**也匹配 `test`。
2. "limit 大于满分候选数就不会 arm"这条，limit 算成 40，实际满分候选是 41 条
   （同样是那个目录）。改成在测试里**数出来**，不靠手算。
3. 第二处 python 替换**静默没匹配上**（我在改 search.rs 的脚本里加了替换计数断言，
   改测试的脚本里忘了），于是 limit 还是 41，测试红了半天。

还有一处：`is_ascii` 守卫在 search 层面本来**一个测试都没覆盖** —— 而它是我自己识别
出的唯一正确性漏洞。补了一条，用单线程 rayon pool 把顺序固定下来，让重音路径
确定性地落到剪枝分支上。

### 量出来的结果

同机 208,156 条语料，20 次 warm 平均（`examples/bench`）：

| 查询 | 命中 | 改前 | 改后 | 倍数 | fzf |
|------|------|------|------|------|-----|
| test | 167,196 | 30.42 ms | **11.00 ms** | 2.8× | 165.5 ms |
| src | 119,010 | 18.39 ms | **7.30 ms** | 2.5× | 121.5 ms |
| index | 37,032 | 11.81 ms | **6.81 ms** | 1.7× | 42.9 ms |
| cargo | 29,421 | 10.14 ms | 8.37 ms | 1.2× | 37.2 ms |
| s | ~155,000 | 12.88 ms | 14.27 ms | — | 175.3 ms |
| seek | 126 | 1.69 ms | 1.63 ms | — | 2.2 ms |

逐键（打 `test`，10 次平均）：

| 键 | 改前 | 改后 |
|----|------|------|
| t | 12.74 ms | 12.69 ms |
| te | 24.54 ms | **22.32 ms** |
| tes | 26.89 ms | 10.51 ms |
| test | 29.37 ms | 10.41 ms |

### 两个诚实的保留

**`s` 没有改善。** bench 里的数字从 12.88 波动到 14.27，看着像退步，但那个 harness
对 `s` 的离散度本来就大（改前四次测量是 12.88／14.98／14.25／13.82）。隔离测量
（每进程只跑一个查询，30 次取 best）说改前 12.99ms、改后 12.68ms —— 没变。我信隔离
测量，因为它排除了 bench 里的顺序效应。
`s` 现在是最慢的查询，但它的 14ms **不在匹配上**：m=1 走 `match_single`，瓶颈是
155k 条候选的 collect 和排序。那是另一条杠杆。

**`te` 现在是峰值键。** 22ms，比 `test` 的 10.4ms 还慢。原因：`te` 是极常见的二字母
组合，绝大多数命中项**确实含有**连续的 `te`，所以剪不掉多少，还要多付一次子串
检测。改前的峰值在 `test`（29ms），现在挪到了 `te`。总体是好的 —— 但"打字越长越快"
这件事在第二、三键之间仍有一个坎。

### 还剩

- `s` 这一类：m=1、命中十几万，成本在 collect 和排序，不在匹配
- `te` 这一类：子串检测不选择的二字母查询
- 外壳那 2ms 固定开销还在，所以 11ms 的 Rust 侧对应约 13ms 的键到屏

## 第 23 步：`te` 的 22ms，以及边搜边出行

第 22 步把峰值从 `test` 的 29ms 挪到了 `te` 的 22ms，并留下一句"第二、三键之间
仍有一个坎"。这一步先把那个坎量清楚，再做一件不依赖它的事：**搜索还在跑的时候
就把已经匹配到的行推给面板**。

### `te` 的 22.02ms 分解

一次键击在 Rust 侧的 22.02ms（208,163 条候选，limit 200）：

| 阶段 | 耗时 |
|------|------|
| 匹配（含剪枝的子串检测） | 13.21 ms |
| 排序键 | 2.59 ms |
| collect | 0.92 ms |
| 选前 N + 取 positions | 5.13 ms |

逐键看匹配那一段，坎的位置很清楚：

| 查询 | m | 匹配耗时 | 增量 |
|------|---|----------|------|
| t | 1 | 4.4 ms | — |
| te | 2 | 13.2 ms | **+8.8** |
| tes | 3 | 16.4 ms | +3.2 |
| test | 4 | 18.0 ms | +1.6 |

`t` 走 `match_single`，**根本不进 DP**。`te` 是第一个进全矩阵的查询，而那 +8.8ms
几乎全是"进矩阵"的固定成本 —— 相位 1 找字符、物化窗口、回溯 —— 不是 DP 那一行
本身（第 22 步量过：每多一个模式字符只 +218ns/命中）。所以这个坎不是渐进的，
是 m=1 → m=2 的一次性台阶。

### 为什么第 22 步的剪枝对 `te` 几乎无效

剪枝的前提是"不含连续子串的路径至少差一个 gap"。`te` 的 196,680 条命中里
**174,098 条（88.5%）真的含有连续的 `te`**，剪不掉；剩下 11.5% 剪掉了，还要为
全部候选多付 1.2ms 的子串检测。

更难受的是另一半：174,098 条含子串的路径里只有 53,156 条真的拿到满分。也就是
**120,942 条（占全部命中的 61%）被完整打了分，却一行也显示不出来**。

### 一个可以做但没做的杠杆：按边界剪，而不是按包含剪

`te` 的上限是 `2×SCORE_MATCH + 2×9 + 9 = 59`，而第二档是 56 ——
`BONUS_BOUNDARY = SCORE_MATCH/2 = 8`，`bonus_boundary_delimiter = 9`。差的那 1 分
把两档彻底分开了，于是有一个精确的刻画：

> 对 `te` 这类查询，**得分 59 ⟺ 模式紧跟在路径分隔符之后（或在偏移 0）**。

把剪枝的判据从"含子串"换成"在边界处含子串"，`te` 的可剪比例会从 11% 跳到约
**73%**。但差距正好是 3，而现在的武装规则是 `ceiling − 2` —— 两者贴得太紧，
改之前必须先把武装阈值和这个新判据一起重证。**所以只写下来，没有实现。**

### 边搜边出行

`te` 的 19.3ms（release）里，第一块 64k 候选只花 4.1ms。既然排名是全局的，
那就承认每一页都是**临时的**，但承认得精确一点：

> 每一次 publish 都恰好是"**到目前为止看过的候选**里的前 N 名"，
> 而最后一次 publish 等于一次普通搜索的结果。

**（订正）**我先前在对话里说过"500 条候选到了满分，列表就可以冻结"。那是错的，
已经丢掉：一个后来出现的满分项如果路径更短，会按 Length 这一档把先出现的挤
下去，所以顺序会一直变。能保证的只有上面那条"每页是前缀的前 N 名"。

三层都改了，但**没有第二套搜索实现**：

- **`core/src/search.rs`**：`search_streaming(corpus, query, options, pool, chunk,
  on_progress)`，按 chunk 分块；`search_in` 变成"chunk = usize::MAX 且回调永远
  `Progress::Continue`"的一个入口。比较器改成接收候选切片做参数（Vec 在块之间
  还在追加，不能被闭包捕获），每块合并前先用当前第 N 名做下限过滤，于是合并量
  只和真正有竞争力的候选成正比。中间页**不带 positions**（那是最贵的一半，
  只在循环结束后为最终页收集一次）。
- **`core/src/ffi.rs`**：`sk_engine_search_streaming` 和 `sk_engine_search` 两个
  薄壳共用一个 `run_search`（照抄本仓库 `search`/`search_in` 的先例）。回调是
  `extern "C" fn(*const FlSearchResult, int32_t, int32_t, void*) -> int`，返回 0
  就放弃搜索。
- **`app/`**：`SearchEngine.search(_:limit:chunk:onProgress:)`，回调通过一个
  `ProgressBox` + 顶层非捕获函数过 C 边界。`AppModel.publishPage(_:matched:page:)`
  收页，`GenerationBox`（NSLock）让后台线程能判断自己是否已被取代。

四件必须想清楚的事，都有测试守着：

1. **放弃的搜索不能留 pool。** pool 是"这个查询的完整命中集"，下一键会拿它代替
   全语料去搜；半截的 pool 会静默丢行。
2. **页的顺序**。每页各自一个 unstructured `Task` 过主 actor，Swift 不保证同优先级
   FIFO —— 所以 `publishPage` 带页号，旧页直接丢，`.complete` 之后的 `.partial`
   也直接丢。
3. **中间页的匹配数不是答案**，它是"到目前为止多少"。直接发出去会让计数器在每次
   搜索里往上爬，并破坏"查询越长命中越少"这个 `loadMoreResults` 正在依赖的不变量。
   所以 `matchedCount` 只在完整答案上动，footer 在"行数超过计数"的那个窗口里
   改说 `N so far`。
4. **解析行是贵的那一半**。把核心返回的行变成 `SearchHit`（全路径、相对路径、
   高亮偏移）要走 `sk_engine_resolve` + `sk_engine_positions_batch`；每页解析 200 行
   给键击加了 20ms。中间页只解析 `partialRows = 32` 行 —— 比面板能显示的
   `panelRows = 14` 宽裕，所以面板高度不会跳。

### 实测（release，208,163 条候选，limit 200，chunk 64k，20 次热身）

分块本身的代价，以及第一页来得多早：

| 查询 | 流式总耗时相对普通搜索 | 第一页 | 整次搜索 |
|------|------------------------|--------|----------|
| t | −0.6 ms | 1.6 ms | 5.2 ms |
| te | +0.7 ms | 4.1 ms | 19.3 ms |
| tes | +0.5 ms | 4.0 ms | 17.6 ms |
| test | +1.3 ms | 2.2 ms | 11.3 ms |
| src | +1.5 ms | 2.2 ms | 7.9 ms |
| index | +0.7 ms | 1.7 ms | 6.5 ms |
| s | −1.1 ms | 4.4 ms | 14.0 ms |
| seek | +0.4 ms | 1.7 ms | 5.4 ms |

每个查询都出 4 页（chunk 数的是候选，不是命中，所以再窄的查询也会分 4 块）。

**真正的收益其实是取消**：一个被下一键取代的搜索，在第一页之后就放弃，
`te` 19.3 → 4.1ms（省 15.2ms）、`test` 11.3 → 2.2ms、`src` 7.9 → 2.2ms。

键到屏的预算：外壳固定开销 6ms（上限 8ms）。

### 我自己写错的四个测试

- **"pool 必须完整"那条用 `test` 当查询。** `test` 在 6,604 条的夹具里命中 4,300 条，
  过半，按设计**本来就不取 pool** —— 断言 `Some(..)` 的是我，不是实现。换成
  `test.rs`（300 条命中）并加了 `matched > limit` 的防空转断言。
- **断言 `scored` 相等。** 两条路径都只跑一块，但 `scored` 是**合法不确定**的：
  剪枝的 `seen` 计数器在 rayon 工作线程间共享，谁被剪掉取决于交错。行不会变，
  `scored` 会。改成 `scored <= matched` 加上行／pool／matched 的相等。
- **断言中间页的路径以根目录路径开头。** 路径确实解析过了 —— 是 macOS 把
  `/var` 符号链接到 `/private/var`，解析后的路径和夹具建出来的 URL 没有公共前缀。
  改成断言"绝对路径 + 后缀等于相对路径 + 文件存在"。
- **两个新测试的前置状态不合法**：`.complete` 之后的 `.partial` 本来就该被丢，
  而一个刚建的 `AppModel` 报的是 `Indexing…`。用现成的 `finishIndexing(count:complete:)`
  测试缝重写了。

另外报错的两个旧测试不是旧测试的错：`PanelRetypingTests` 在半成品计数上取样
（修法是 `countIsFinal`），`IdleRefreshTests` 被用户自己那次搜索的后续页重新填了
列表 —— 于是加了 `@Published isSearching`。这个字段的意义超出测试：有了流式，
"屏幕上有行"不再等于"搜索结束了"。

### 守着它的测试

- `core/tests/streaming.rs`（11 条）：夹具**故意把最好的匹配放在最后**（4000 条
  散开的 `t-e-s-{i}-t.md`、2000 条填充，然后 300 条满分的 `best/d{i}/test.rs`），
  并有一条显式的前置条件测试守着这个布局。最吃重的一条把每次 publish 和
  `rank_candidates` 跑前 `(publish+1) × CHUNK` 条路径的结果逐字段对比。
- `core/tests/ffi_streaming.rs`（5 条）：过函数指针和 context 指针走真 C ABI，
  包括"被放弃的搜索不能毒化下一次搜索"。
- `app/Tests/SeekTests/StreamingSearchTests.swift`（7 条）：报上来的页是
  **可画的**（绝对路径、名字、相对路径、非空高亮）。
- `app/Tests/SeekTests/StreamingSelectionTests.swift`（9 条）：选中跟着用户
  选的那一行按 id 走、旧页被丢、中间页不动计数、footer 不吹它没有的数。

170 条 Rust 测试、0 warning、8/8 差分门全绿；核心流式逻辑上的 5 个变异（放弃的
搜索留 pool、忽略 `Stop`、页满后不合并、发未排序的页、丢掉运行计数）5/5 变红。
Swift 侧 96 条 1 失败 —— `PanelFocusTypingTests` 的那条顺序依赖，单独跑是绿的。

## 第 24 步：按边界剪枝（`te` 20.6 → 10.3ms）

第 23 步写下了杠杆但没实现：把剪枝判据从"含连续子串"换成"在分隔符后含连续子串"。
这一步做了，并且把那句"差距正好 3，贴得太紧"真正重证了一遍 —— 结论是margin 够，
但**有一类查询必须排除**，而我差点漏掉它。

### 论证（写在 `ceiling_boundary` 的文档里）

上限是 `m×16 + 2×maxB + (m−1)×maxB`。关键在 DP 的这一行：

```rust
if b >= BONUS_BOUNDARY && b > fb { consecutive = 1; } else { b = b.max(BONUS_CONSECUTIVE).max(fb); }
```

连续串里后面的字符**继承串首的 bonus** `fb`。所以无间隙对齐的得分是
`m×16 + 2×b₁ + Σ_{i≥2} max(ownᵢ, 4, b₁)`。于是：

- `b₁ = maxB` 需要前驱字符的类别恰好付最高 bonus。Path scheme 下
  `bonus_boundary_delimiter = 9 > BONUS_BOUNDARY = 8 = bonus_boundary_white`，
  所以只有 `/`（以及偏移 0，因为 path scheme 的 `initial_char_class` 就是
  `Delimiter`）。Default scheme 下最高的是 `bonus_boundary_white = 10`，所以是空白。
- `b₁ ≤ maxB − 1` 时，若模式自身的字符都付不出 maxB，则 `ownᵢ ≤ 7`（camelCase）
  或 0，于是每个 `bᵢ ≤ maxB − 1`，**亏空 ≥ 2 + (m−1) = m + 1 ≥ 3**。

所以：**得分 ≥ 上限 − 2 ⟹ 存在一个紧跟分隔符（或在偏移 0）的连续串**。剪枝的
武装阈值正是 `ceiling − 2`，所以被剪掉的候选确实进不了前 N 名。

### 差点漏掉的那一类：模式自带边界

`m + 1 ≥ 3` 的前提是"模式自身的字符付不出 maxB"。反例是 **`/b`**：

`x-/b` 里这个串前面是 `-`（NonWord → b₁ = 8），但 `b` 的前驱是模式自己的 `/`
（Delimiter → own = 9 = maxB）。得分 = (16+2×8) + (16+9) = 57 = 上限 59 − **2**。

正好等于武装阈值，也就是正好可能和最后一行并分、再靠 Length 赢过它。所以
`ceiling_boundary()` 对这类模式返回 `None`，判据退回原来的"含子串"。检查是
精确的、从 bonus 矩阵里查的：**模式任意相邻两字符（含大小写变体）之间的 bonus
若 ≥ maxB，就拒绝**。那条 57 = 上限 − 2 的断言本身就是一个测试，并带着
"如果这个数变了，下面那条守卫要重新推导"的注释。

边界字节集也不是写死的常量，而是**从 bonus 矩阵推出来**的：扫 128 个 ASCII
字节，凡其类别能给模式首字符付出 maxB 的就置位，存成 u128 掩码（每次键击每条
候选都要查一次）。这样改 scheme 的常量时集合会跟着动，而不是悄悄失效。

### 实测（release，208,164 条候选，limit 200，每查询 20 次热身，各 3 次采样）

| 查询 | 改前 | 改后 | 剪掉比例 |
|------|------|------|----------|
| te | 20.20 / 20.47 / 21.12 ms | **9.60 / 10.23 / 11.21 ms** | 11% → **72%** |
| tes | 11.09–11.36 | 10.32–10.87 | 75% → 76% |
| test | 11.74–12.72 | 10.81–12.00 | 73% → 75% |
| src | 7.40–8.04 | 6.72–7.04 | 79% → 86% |
| index | 7.51 | 6.41–6.78 | 66% → 后者波动大 |
| do | 7.31–7.49 | 6.38 | 83% → 90% |
| t | 12.36–15.03 | 12.36–15.03 | 0%（m=1，剪枝关着） |
| s | 13.92–17.13 | 13.92–17.13 | 0%（同上） |

`te` 2 倍。`t` 和 `s` 走的是同一段代码（`ceiling_boundary` 对 m=1 返回 `None`），
两次采样的区间完全重叠 —— 那 12–17ms 的跳动是这个 harness 对 m=1 的离散度，
不是回归。

**峰值键换人了**：`te` 不再是最慢的，现在是 `s`/`t` 这一类的 14ms，而它们的成本
不在匹配（m=1 走 `match_single`），在 20 万条候选的 collect 和排序。

bench 的查询列表里加了 `te`，因为它现在是"最贵的形状"的代表：两个字母、要走全
矩阵、而且常见到几乎每个命中项都含它。

### 测试

- `core/tests/score_bound.rs`（5 → 13 条）：承重的那条在整个语料上断言"无边界串
  ⟹ 得分 ≤ 上限 − 3"，并带 `checked > 0` 防空转；逆向那条在**手写**的路径集上断言
  双向等价（语料里几乎没有任何模式能打满分 —— 每个文件名都带索引前缀 —— 所以在
  语料上做逆向会空转通过）；再加 scheme 差异（Default 下 `/` 不是边界、空白才是）、
  自带边界、单字符、非 ASCII 四条守卫。
- `core/src/fuzzy/literal.rs`（7 → 13 条）：`contains_after` 的单元测试，包括"在
  边界上但不匹配时不能停止扫描"（`t/test.md`）和"非 ASCII 保守答 true"。
- `core/tests/boundary_prune.rs`（新，5 条）：语料里**每一条命中都含连续串**，只有
  1/101 在分隔符后。前置条件测试守着这一点（否则这个文件就变成在重测
  `top_rows_pruning.rs`）。差分对比 `rank_candidates`、匹配数不变、pool 完整。
  这条测试在改之前是红的：`scored == matched`。

189 条 Rust 测试全绿、0 warning、8/8 差分门绿、Swift 96 条 1 失败（还是
`PanelFocusTypingTests` 的旧顺序依赖）。新逻辑上的 6 个变异 6/6 变红：偏移 0 不算
边界、把 `>= max` 写成 `> max`（放过 `/b`）、所有字节都算边界、不看 `follows`、
测串后面那个字节而不是前面那个、单字符模式也剪。

## 第 25 步：量 `s` 的 14ms（只量，没改）

第 24 步之后 `s`／`t`／`e` 这一类是唯一剩下的慢查询。先量再动，所以这一步只有
测量和三条杠杆的取舍，代码一行没改。

### 分解（release，208,164 条候选，limit 500，每相 20 次热身，3 次采样）

探针按阶段叠加地量，每一阶段都包含前一阶段：

| 阶段 | `s` | `t` | `e` | `te`（对照） |
|------|-----|-----|-----|--------------|
| 0 只走一遍路径 arena（并行） | 0.26 ms | 0.26 | 0.39 | 0.20 |
| 1 只匹配（并行） | 3.2 ms | 5.6 | 3.0 | 15.4 |
| 1 只匹配（单线程） | 17.9 ms | 22.3 | 17.8 | 95.3 |
| 2 ＋算排序键 | 5.6–6.4 ms | 8.4 | 5.8 | 17.2 |
| 3 ＋collect 成 Vec | 6.1 ms | 9.3 | 6.6 | 19.5 |
| 4b 合并（现在的做法） | **4.7–4.8 ms** | 3.7 | 3.9 | 3.0 |
| 6 取显示行的 positions | 0.04 ms | 0.04 | 0.04 | 0.22 |
| 5 真实搜索（单块） | 13.3–16.1 ms | 14.0 | 16.4 | 12.4 |

所以 `s` 的 ~14ms 大致是：**匹配 3.2 ＋ 排序键 2.4 ＋ collect 0.6 ＋ 合并 4.8**，
其余是噪声和一次性开销。`match_single` 不走 DP，12.6ns/条，这部分几乎到底了；
走 arena 的地板是 0.26ms。单线程 17.9ms / 并行 3.2ms = 5.6 倍（8 核）。

### 发现：最大的一块是"合并"，而那是第 23 步引进的

分块之后，每块合并前先取一次下限：

```rust
let floor = (best.len() >= options.limit).then(|| best[best.len() - 1]);
```

`search_in` 只有一块，进来时 `best` 是空的 ⇒ `floor = None` ⇒ **205,652 条全部
push 进 `best`，然后对 20 万条跑一次 `select_nth_unstable_by`**，比较器每次要
随机读两次 8.2MB 的候选数组。

把下限改成**在 push 循环里随时更新**（攒到 2×limit 就 select_nth 一次、截断、
把第 limit 名设成新下限），同一份候选数据上量：

| 查询 | 现在（push 全部再选） | 随时更新下限 |
|------|----------------------|--------------|
| s | 4.80 / 4.81 / 4.69 ms | **1.37 / 1.39 / 1.38 ms** |
| t | 3.71 | 1.43 |
| e | 3.88 | 1.37 |
| te | 2.99 | 1.29 |
| test | 2.53 | 1.05 |

对 `s` 省 3.4ms。这不改排名语义 —— 比较器是同一个，下限过滤本来就在代码里，
只是算的频率从"每块一次"变成"每次 push"。两者都在已 collect 好的热数据上量，
而真实搜索是刚写完 8.2MB 的冷数据，所以这个数字如果有偏，是**低估**。

要注意的是：**面板（chunk 64k）本来就没有那么糟** —— 第一块之后 `best` 已满，
第 2–4 块是过滤过的 push。所以这条杠杆对面板只省第一块那 64k 的部分，不是全部
3.4ms。探针里 `search_in`（单块）和 `search_streaming`（64k）的总时间差在噪声
以内，也印证了这一点。

### 否掉的一条：m=1 也用边界剪枝

第 24 步的边界判据对 m=1 **不是**"包含即匹配"那个退化情况：`s` 的上限是
`16 + 2×9 = 34`，第二档是 `16 + 2×8 = 32`，亏空 2 —— 所以武装阈值得从
`ceiling − 2` 收紧到 `ceiling − 1`（m=1 的分数只有偶数档，`≥ 33` 等价于 `== 34`），
论证是成立的。

但量下来**不值得**：

| 查询 | 命中 | 有边界出现的 | 可剪 |
|------|------|--------------|------|
| s | 205,652 | 173,234 | **15.8%** |
| t | 202,809 | 114,757 | 43.4% |
| e | 205,585 | 115,807 | 43.7% |
| z | 19,201 | 8,579 | 55.3% |

`s` 只能剪 16% —— 因为"路径里有任何一段以 s 开头"就算边界出现，而 `src/`、
`Sources/`、`scripts/`、`site-packages/` 之类太多了。探针里模拟这条剪枝
（匹配＋边界检测＋只给留下的算键）对 `s` 是 7.60ms vs 不剪的 6.06ms，**净亏**；
`t` 省 2.5ms、`e` 亏 0.8ms，在噪声里。所以这条对 `s` 这一类不是杠杆。

### 没做的结构性一条

真正的结构问题是：**为了显示 500 行，collect 了 205,652 条 40 字节的候选（8.2MB）
再合并**。让每个 rayon worker 自己维护一个 top-N，最后只合并 8–16 个小堆，能同时
去掉这 8.2MB 和那次巨型 select_nth（估计再省 4–5ms）。但它要同时照顾两件事：
`matched` 计数，以及收窄用的 pool（那个需要**全部**命中的下标，不过只要 4 字节
一个，不是 40）。这是搜索数据流的重写，而且要和第 23 步的分块发布重新对齐
（每块边界上合并各 worker 的堆）。先记下来。

## 第 26 步：随时更新下限（`s` 12.0 → 9.4ms），以及一个流式带来的回归

第 25 步量出"最大的一块是合并，而且是第 23 步分块时引进的"。这一步做了那条。
中途用户报了一个症状，先说那个 —— 它也是第 23 步引进的。

### 回归：上一个查询的页会把行放回屏幕

用户的话是："输入 `ha`，列表里是 `hash`、`har` …… 接着打成 `hash`，列表里就该
只剩 `hash` 的结果"。

流式搜索在块边界检查世代号、然后放弃；但**已经交给 main actor 的那一页还在路上**，
而 Swift 不保证同优先级的 unstructured task 按创建顺序跑。`publishPage` 当时的
签名里没有世代号，它把 `self.generation`（也就是**新**的那个）传给 `publish`，
于是旧查询的行被当成新查询的答案贴了上去。更糟的是 `lastPage` 也被带到了旧搜索
的页号上，于是**新搜索自己的第 0 页反而被当成"更旧的页"丢掉**，一直到
`.complete` 落地才纠正。

修法：`publishPage(_:matched:page:from:)` 多一个"这一页属于哪个世代"，第一行就
守住。守卫放在 `publishPage` 而不是 `publish` 里是故意的 —— `lastPage` 的记账
也必须一起不动，否则就是上面那个"新页被丢掉"的情形。顺手把三处 `.complete` 的
调用也改成显式传世代号。

测试 `test_a_page_from_a_superseded_search_is_dropped` 在改之前是红的，而且红在
第二条断言上（行本身因为参数改名已经被 `publish` 的旧守卫挡住了，但页号的污染
没有）—— 这正好说明两件事都要修。另外加了
`DynamicNarrowingTests`：真实索引上打 `ha`（188,565 命中）再打 `hash`
（103,062 命中），等搜索落定后断言**每一行**都是 `hash` 能匹配的；它还先断言
`ha` 的行里确实有 `hash` 匹配不到的，否则这条测试分不出两者。

要说清楚的一点：**从按下键到第一页落地的那几毫秒里，旧行是故意留在屏幕上的**
（否则每敲一下都闪一下空列表）。所以"列表里只剩 `hash`"是"搜索落定后"的性质，
不是每一帧的性质。还有，footer 的 103,062 是 fzf 意义上的匹配数（子序列），不是
"含 `hash` 的文件数"；要字面子串得用 `'hash'`。

### 随时更新下限

```rust
let floor = (best.len() >= options.limit).then(|| best[best.len() - 1]);
```

`search_in` 只有一块，进来时 `best` 空 ⇒ `floor = None` ⇒ 20 万条全 push，再对
全部跑一次 `select_nth`。改成在 push 循环里维护：攒到 `2 × limit` 就
`select_nth(limit − 1)`、截断到 `limit`、把第 `limit` 名设成新下限。

两个细节：
- **不是一超过 limit 就修剪**。那等于每条候选跑一次选择，第 25 步的探针里比
  "完全没有下限"还慢。攒到两倍再修一次，把一次选择摊到 `limit` 次 push 上。
- 顺手修了一个潜伏的下溢：`limit == 0` 时原来的表达式会算 `best[0 - 1]`。FFI 那
  一侧 `capacity <= 0` 挡着，所以没人踩到过，但它就在我要改的这个表达式里。

### 实测（release，208,164 条候选，limit 500，A/B 交错各 3 次，取最小值）

| 查询 | 改前 | 改后 | 差 |
|------|------|------|-----|
| s | 12.01 ms | **9.42 ms** | −2.59 |
| t | 12.39 | **9.68** | −2.71 |
| e | 12.46 | **9.81** | −2.65 |
| te | 9.76 | 9.05 | −0.71 |
| tes | 9.50 | 9.14 | −0.36 |
| test | 10.34 | 10.01 | −0.33 |
| src | 6.28 | 6.48 | +0.20 |
| index | 5.48 | 5.71 | +0.23 |
| z | 2.03 | 1.81 | −0.22 |
| seek | 1.49 | 1.46 | −0.03 |

交错采样是必须的：单独各跑一次时 `s` 看起来只省 0.6ms、`src` 看起来退步 1.7ms，
全是噪声。交错之后"改后"那一列的离散度明显更小（`s` 三次都是 9.4-9.5）。

`src`／`index` 的 +0.2ms 在采样区间里重叠（src 改前 6.3/6.8/6.9、改后
6.5/7.1/6.6），当成持平。它们是剪枝最有效的查询（剪掉 85%／66%），进入合并的
候选本来就少，下限很少填满，所以只付了每次 push 多一个比较的钱。

**现在所有键都在 10ms 附近**，第 25 步说的"`s` 这一类是唯一的慢查询"这件事到此
为止 —— 峰值从 14ms 降到 9.8ms。

### 一个诚实的保留

这条优化**没有观测点**。第 22 步的剪枝有 `scored` 可以断言"确实剪了"，而合并的
下限没有对应的计数器：3 个会丢行的变异（下限取成第一名、选择保错一端、截断少一行）
都变红，但"干脆不更新下限"这个**纯性能**变异是绿的 —— 它仍然是正确的，只是慢。
所以这条的证据只有钟表。没有为此加计数器，因为那是为测试而加的产品代码。

189 条 Rust 测试、0 warning、8/8 差分门绿；Swift 98 条（新增 2 条）1 失败，还是
`PanelFocusTypingTests` 的旧顺序依赖。

## 第 27 步：预览面板（以及一个抓不到的截图）

三个选择定下来：按键切换（Tab）、QuickLook 全类型、面板变宽预览在右。

**几何。** 面板原来只会跟着内容改高度，`resizePanel` 收一个 `CGFloat`。
预览列让宽度也变成动态的（720 ↔ 1080），所以那段算术抽成了纯函数
`PanelPlacement`：`summoned` 给出召唤时的位置（顶边放在 `midY + 460×2/3`），
`resized` 保持顶边和水平中心不动，再把 x 夹回屏幕。选"中心不动"而不是
"左边不动"：1080 的面板只向右长，在 1512pt 宽的屏上右边缘几乎贴边；代价是
列表在切换的那一刻左移 180pt，但那是用户自己按的键。宽于屏幕时居中，
两边各丢一点，而不是一边全丢。

抽成纯函数的收益是它第一次能被测：顶边不动、中心不动、被推出边缘会拉回、
太宽时居中 —— 六条测试，不需要窗口也不需要显示器。

**预览列只在有选中行时才画。** 设置是记住的，所以面板会带着"要预览"打开，
而这时查询是空的。没有这条，空查询会得到一个 460pt 高的面板配一个空盒子。
加了之后，空查询的面板和没有预览时一模一样。

**开着预览时面板固定 460pt 高。** 三行结果配一个 60pt 高的预览框没用，
而一个每次按键都改高度的预览框更糟。代价是"面板和内容一样高"这条规则
在预览模式下不成立。

**防抖 120ms。** 按住方向键连划二十行会触发二十次 QuickLook 加载来显示
第二十一行。`PreviewController` 里 `target` 发布的每一个值都是一次加载，
所以测试直接断言发布流：只经过没停下的行一次都不许出现。另外用一个
`shown` 字段挡住重复 —— 流式搜索每页都会把选中行重新交一遍，不挡的话
预览会跟着每一页闪。元数据（大小、修改时间）在 `Task.detached` 里取。

**设置窗口的三个灯。** 面板本来就没有窗口按钮（`.titled` 不带 `.closable`
就不会生成），但设置窗口有 `.closable, .miniaturizable`，三个灯都在。
探针确认：面板三个按钮 `exists=false`，设置窗口三个都 `hidden=false`。
改成 `styleMask = [.titled]`，关闭走视图自己的 Done（`close()` 不需要
`.closable`，只有 `performClose` 需要）。

**一个抓不到的截图。** app 的 `--snapshot` 走 `cacheDisplay(in:to:)`，
它抓不到进程外的视图。探针打出视图树解释了快照里看到的东西：

| 文件 | QLPreviewView 的子视图 | 快照里 |
|------|----------------------|--------|
| PNG | `NSRemoteView` | 空白 |
| Markdown | `QLDisplayBundleContainerView` | 画出来了（是类型图标） |

图片和 PDF 走 XPC 的远程视图，`cacheDisplay` 拿不到，所以快照是空的而真实
面板里有内容。Markdown 走进程内的 display bundle，能被抓到 —— 而它画出来的
就是那个蓝色的 M↓ 图标：这台机器上系统没有 Markdown 的文本生成器，Finder
按空格也是同样的结果。**这是 QuickLook 的诚实回答，不是 bug**，但对一个
满是 `.md` 和源码的语料来说，它意味着最常见的文件预览不出内容。

**没有观察点的那部分：** 我自己看不到真实窗口（终端没有屏幕录制权限），
所以"图片和 PDF 真的渲染出来了"只有视图树作证，没有像素作证。

### 文本自己渲染

`.md` 出图标这件事，对一个满是笔记和源码的语料来说就是"最常见的文件预览不出
内容"，所以文本改成自己读自己画，其余仍交给 QuickLook。

判定分两步，顺序不能反：**先问文件自己声明的类型，再看字节**。只看字节不行 ——
PDF 的开头可以连着好几页都是纯 ASCII，字节会把它判成文本，然后给用户画一屏
`%PDF-1.4` 和对象定义。只看类型也不行 —— `Makefile`、`LICENSE`、`.gitignore`
这些启动器天天撞上的文件没有扩展名，`UTType` 认不出来，只能靠字节（头部没有
NUL 且能 UTF-8 解码）。富文本和 HTML 虽然 conform 到 `.text`，但排版就是它们本身，
留给 QuickLook。

只读头部：64KB，再截到 400 行。**两道上限都在干活**，这是变异测试逼出来的：

| 变异 | 第一版测试 | 收紧后 |
|------|-----------|--------|
| 去掉类型判定 | 红（PDF 被当成文本） | 红 |
| 去掉 NUL 判定 | **活** —— fixture 叫 `picture.png`，类型判定先把它挡掉了，根本走不到 NUL 那行 | 红（fixture 改成无扩展名的 `linker`） |
| 去掉行数上限 | **活** —— fixture 是 2.4MB 的长行文件，64KB 字节上限先截住了，而断言只写了"小于原文件 1/4" | 红（fixture 改成 3 万个单字符行，断言改成行数 ≤ `headLines`） |

两个存活都是同一类错误：**fixture 的形状让被测的那一行代码根本不在执行路径上**。
新 API 靠编译失败变红是比断言失败弱的红，这次正好证明了弱在哪。

**发布结构也改了。** 原来 `target` 和 `metadata` 是两个 `@Published`，
目标先发、元数据后到，header 会先对着新文件画出旧文件的大小。现在合成一个
`FilePreview`（url + metadata + body），整个在 detached task 里算完一次发布。
顺带发现 `Preview` 这个名字和 SwiftUI 自己的类型撞了。

## 第 28 步：屏幕上的行属于输入框里的查询

用户截图：输入框写着 `nature`，footer 是 `40,155 matches`（`nature` 的数），
预览列画着 `nature-writing`，而**结果行还是 `hel` 的行，连高亮都还是 `hel` 的**。
一句"this is critical issue never fixed"——它从第一个 commit 起就在。

### 真因：`ForEach` 的身份被 `.id(index)` 顶掉了

```swift
ForEach(Array(model.results.enumerated()), id: \.element.id) { index, hit in
    ResultRow(hit: hit, isSelected: index == model.selection)
        .id(index)          // ← 这一行
```

`ForEach` 已经用文件 id 做身份了。行上再加 `.id(index)` 不是"补一个"，是
**替换**：SwiftUI 记住的身份变成了 0...199。换一个查询，就文件 id 而言是
"全删 + 全插"，就索引而言却是**一个都没变**，于是 `LazyVStack` 判定这些行
还是原来那些行，把已经建好的视图留在原地。

模型是对的，body 也确实跑了新数据——这正是它难抓的原因：所有模型层测试都
绿，`ImageRenderer` 快照也绿，因为新建的层级不可能"留着"旧视图。

滚动条目的 `scrollTo` 跟着改成按文件 id 滚，不再按下标——身份只剩一套。

### 走过的两条弯路（都记下来，因为它们各自是一类错误）

**第一条：假红。** 第一版复现测试比较两张 PNG 的**字节全等**。
修好之后它照样红：140,636 vs 140,670 字节——34 字节，画面其实一模一样，
差的是滚动条自己淡出的那一帧。靠这个断言做的那次"`.id(index)` 不是真因"
的证伪**整个作废**，因为两版都红，它根本区分不了。
断言换成**差异像素占比**之后才有分辨力：

| | `hel` → `nature` 画面变化 | 画面 vs 模型该画的 |
|---|---|---|
| 有 `.id(index)` | **0.09%**（等于没变） | 5.1% / 10.8% |
| 去掉 | 大幅变化 | 噪声级 |

**第二条：测试把用户的偏好写脏了。** `togglePreview()` 会写进
`Preferences`，而 `UserDefaults` 是**跨进程跨次运行**留下来的。探针开过预览
之后，`showPreview=1` 同时留在了 `dev.seek.app`（app 的域）和
`com.apple.dt.xctest.tool`（测试进程的域），于是 `PanelHeightTests`
和 `AppDelegateIntegrationTests` 一起红——"三个结果和两百个一样高"、
"面板超过了天花板"。两条都不是几何回归，是预览被打开了。
`StaleRowsTests` 现在在 `setUp`/`tearDown` 里存回原值。

### 这个 bug 只有活的层级能抓

测试必须**照搬 `makePanel`**：`AnyView` 根、`panel.contentView = hosting.view`
（不是 `contentViewController`）、同样的 styleMask。而且要有两种场景：
列表停在顶部，和**已经滚下去**——`LazyVStack` 得先建过一批视图才有东西可留。

### 没能用上的仪器（记下来省得再试）

| 想读的东西 | 工具 | 结果 |
|---|---|---|
| 窗口服务器合成的像素 | `CGWindowListCreateImage` | 没有屏幕录制权限，58 个窗口只有 4 个可读标题 |
| 窗口 backing store | `lockFocus` + `NSBitmapImageRep(focusedViewRect:)` | layer-backed 的 SwiftUI 视图上返回 nil |

能用的只有 `cacheDisplay`——它是**要求视图现在重画**。所幸身份冲突留下的是
视图本身的旧内容，重画也照样是旧的，所以它够用。

## 第 29 步：一张截图里的三件事

用户搜 `nihao`，截了一张图说"你肯定能发现问题"。逐条查下来，三件事里只有两件是 bug。

### 一、选中行的高亮是看不见的（真 bug）

截图第一行 `Hao Law NIW EB1 Documents List Please Read.pdf` 整行白粗，
一个蓝字都没有，而它下面每一行都有蓝色高亮。

```swift
if isSelected {
    piece.font = .system(size: size, weight: .bold)
    piece.foregroundColor = .primary      // ← 颜色被丢掉了
}
```

当初的理由是"行本身已经有 tint，accent 叠 accent 是一团糊"。问题是选中行的
名字**本来就是 semibold**，于是高亮成了 13.5pt 下的 bold 对 semibold ——
等于没有。**排在第一的那一行，恰好是唯一看不出匹配了什么的那一行。**

把选中行按和其他行一样的方式高亮即可：选择条只有 22% 的 accent，accent 本色
压在上面，深浅两种外观下都清楚（两张渲染图都看过了才定的）。

### 二、`'nihao` 是个死胡同（真 bug，而且是 fzf 对齐问题）

用户后来补了一句："我没有在搜索框中使用 regex,结果中却显示这样"。

先说清楚：**这不是 bug。** `fzf --filter=nihao` 返回的是同样的文件 ——
n、i、h、a、o 按序出现在路径里就算匹配，这就是 fzf 的默认语义。

但用户想要的东西在 fzf 里是有的：**前导单引号，不闭合**。

```
fzf --filter="'nihao"   →  notes/NiHao.txt, docs/nihao-world.md
fzf --filter="nihao"    →  上面两个，再加 src/ni_hao.rs、那个 PDF、minimal_hanoi.py
```

而我们的解析器要求成对引号，`'nihao` 走的是 `UnterminatedQuote`。
更糟的是 FFI 里有**两处**兜底把这个错误吞掉：

```rust
Err(QueryError::UnterminatedQuote) => return 0,   // 查询检查：不报错
Err(QueryError::UnterminatedQuote) => {           // 搜索：去掉尾部引号再试
    match Query::parse(text.trim_end_matches('\'')) { ... Err(_) => return 0 }
}
```

`'nihao` 的引号在**前面**，`trim_end_matches` 剪不掉，于是 parse 再次失败、
`return 0`。**打 `'nihao` 得到的是一个空面板，而且不给任何理由。**

改法：有闭合引号就成对读（`'hello world'` 这个能放空格的扩展保留），没有就
读到空白为止 —— 也就是 fzf 的写法。`UnterminatedQuote` 整个删掉，连同 FFI
里那两处兜底，因为它们现在没有能触发的输入了。

### 三、"first 400" —— 是 bug，在流式分块和过期计数的夹缝里

页脚写 `174,504 matches · first 400`，而列表停在第一行。`loadMoreResults`
的注释说得明白："it is paid when the user scrolls, never on a keystroke"。

**离屏宿主的面板复现不出来**：逐字符打 `nihao`，`onAppear` 走到的最大下标是
**14**，`results.count` 一直是 200。我据此一度判定它不是 bug —— 那是错的。
debug build 太慢，中间那几个查询（`n`、`ni`…）在两次敲键之间根本跑不完，
`matchedCount` 始终是 0，那道闸门压根没被走到。用户跑的 release build 每次都跑完。

同步的模型测试把它钉住了。两个半边都很普通：

* `matchedCount` 只在搜索**完成**时才写（这样页脚不会报出一个比底下列表还小的
  总数），所以下一个查询还在流式返回的时候，手里的计数是**上一个查询的**；
* 流式搜索分块发布，早期的块很短 —— 实测 32 条。

合起来：32 条块里的第 14 行满足"距末尾 20 行以内"，而过期的计数说后面还有
17 万条可取。于是没人滚动过的列表自己把结果集翻了一倍。

改法：`shouldLoadMore(forRow:)` 和 `loadMoreResults()` 两处都加 `isComplete`
闸门 —— 只有这个查询自己的答案到齐了，才算数得清末尾在哪。

真机验证（release bundle，粘贴 `nihao` 一次性触发同一条路径）：页脚
`160,388 matches · first 200`。

### 四、`PanelFocusTypingTests` 的顺序依赖

单跑绿、全套红，137 个测试里唯一的失败。诊断打印把它说穿了：

```
FOCUS responder=…SystemTextFieldFieldEditor…   ← 发通知之前，焦点在搜索框
FOCUS after-notify responder=Optional(<NSPanel: …>)   ← 发通知之后，掉回面板
```

**焦点不是没拿到，是发出通知之后被弄丢的。** `.seekFocusSearch` 是进程级
广播，进程里每一个活着的 `ContentView` 都答应它、把自己的 `isSearchFocused`
置真。前面测试类 `makeKeyAndOrderFront` 过的面板被 AppKit 的窗口列表一直留着，
于是一次广播同时打到好几块面板上，这一块的 field 反而辞掉了 first responder。

两个类一起跑就能确定性复现：`swift test --filter
"PanelDeletingTests|PanelFocusTypingTests"` 必红，签名一字不差。而
`PanelFocusTests` 配它是绿的 —— 那个类的面板里装的是 `Color.clear`，根本不监听。

改法：这条测试要求自己是进程里唯一的面板，就在 `setUp` 里把这件事做实（连
contentView 一起摘掉，里面的 SwiftUI 视图才会停止监听），`tearDown` 里对称地
收掉自己的，不把自己拒绝继承的东西留给下一个类。app 只有一块面板，所以真机
上碰不到这条。

全套 137 个测试 0 失败。

### 仪器又错了一次（第二次了）

第一版高亮测试比较"有匹配 vs 无匹配"两张图差了多少像素。结果**坏版本的分数
更高**（0.055 对 0.031）：semibold 换 bold 会让后面每个字符重排，动的像素很多，
可是一个字也没告诉用户。

换成量**非灰度墨水像素的占比**才对：文字在任何字重下都是白或灰，匹配字符是行
里唯一带颜色的东西，重排伪造不出来，而且不预设 accent 是什么颜色（那是用户在
系统设置里选的）。坏版本在这把尺子下是**精确的 0**。

和第 28 步那次"字节全等"是同一类错误：**量到了变化，但量的不是要求。**

### 五、README 里跑不通的两条命令

`swift run Seek --smoke-test ~/projects` 在一个**完全健康**的索引上报 FAIL：

```
indexed 143 entries          ← 和 `fd . ~/projects` 的 142 条对得上
query "projects" -> 0 hits
FAIL: the query returned nothing
```

扫描是对的，坏的是查询词。SmokeTest 拿 root 的最后一段当"一个肯定存在的
名字"，可是 matcher 打分用的是**去掉 root 前缀的相对路径**（`SearchHit.relativePath`
的注释早就写明了），所以 root 自己的名字恰恰是唯一一个**保证不在索引里**的名字。
这条命令从写下来那天起就没通过。

改法：查询词改成从索引里拿 —— 空模式像在 fzf 里一样匹配一切，所以它排出的
第一行就是本索引的真实条目；只留名字里的字母和数字，因为名字可能带引号或
叹号，那是查询操作符而不是要找的字符（过滤不改顺序，所以它来自的那条仍然匹配）。
现在输出 `query "seek" -> 142 hits` … `OK`。

另一条：第 62 行那个 `cargo test --test fzf_parity` 没有 `cd core`，而仓库根目录
没有 `Cargo.toml`。补上 `cd core`，顺带把代码块标上 `sh`。

README 里的命令逐条跑过：`cargo test`（191 passed / 0 failed / 8 ignored）、
`cargo test --test fzf_parity -- --ignored`（3 passed）、
`cargo run --release --example bench -- ~/`、`swift test`（137 passed）、
smoke test（OK）。

## 第 30 步：排序在中文路径上和 fzf 分了家

用户打 `hellp`，抱怨"为什么 h 开头的都显示出来"。

**先把他问的那件事量清楚**：把 app 看到的那份语料（107 万条）同时喂给真 fzf 和
这个实现，`hellp` 两边都匹配 **231,123 条**，集合完全相同。那些 `hello-world…`、
`help_large.png`、`__hello__.py` 是 fzf 自己认的——`h,e,l,l,p` 按序出现在整条
路径里就算匹配，这是 fzf 的语义，不是 bug。

**但顺序不一样。** 46,460 行错位。而且错位是有规律的：

```
纯 ASCII 的 226,911 行，相对顺序和 fzf 逐字节一致
所有分歧都落在含非 ASCII 的那 4,212 行上
```

### fzf 自己把字节和字符混了

`src/result.go`（v0.70.0）的 `byPathname`：

```go
lastDelim := -1
s := item.text.ToString()
for i := len(s) - 1; i >= 0; i-- {
    if s[i] == '/' || s[i] == '\\' { lastDelim = i; break }
}
if lastDelim <= minBegin { val = util.AsUint16(minBegin - lastDelim) }
```

`s[i]` 索引的是 Go string，也就是**字节**；`minBegin` 来自匹配偏移，是
**rune 索引**。纯 ASCII 下这两个数相等，所以这件事永远不会暴露。路径里有多字节
字符就不等了，而且两个方向都看得见：

* `a/я/hellp.txt` —— `я` 两字节，分隔符在字节 4，匹配起点在字符 4，差值 **0**，
  比任何 ASCII 路径（差值 1）都排得靠前；
* `a/背/hellp.txt` —— `背` 三字节，分隔符在字节 5，匹配起点在字符 4，
  `lastDelim > minBegin`，被当成"匹配跨过了分隔符"，直接判到最差档。

这个实现两个数都按字符算——那是**讲得通**的读法，也是我原来写的。它让
`a/背/hellp.txt` 排第一，而 fzf 把它排第三。**和 fzf 不一致的排序就是 bug，
不管哪一边更讲得通**，所以这里照抄 fzf 的混用。

### 为什么这个 bug 能活到现在

parity 测试的两个盲区正好交叉：

1. `our_order` 写死 `Scheme::Default`，fzf 也没加 `--scheme=path` —— **app 真正
   跑的 path scheme 从来没和 fzf 对过**；
2. 语料取自仓库和 `/etc`，**每个字节都是 ASCII**，而这个 bug 只在多字节字符上显形。

两个盲区各自都不致命，叠在一起就把一条真 bug 藏了下来。补法：scheme 贯穿整个
harness（两种都跑），语料加进一组多字节路径（2/3/4 字节、分隔符前后都有），
再加一条 `parity_over_multibyte_paths`。

### 验证

* 单元测试 `the_pathname_tiebreak_indexes_the_separator_in_bytes`：期望值是
  `fzf --filter=hellp --scheme=path -i` 的真实输出，不是我推的；
* 变异验证：把 `last_delim = byte as i32` 退回 `count`，这条单元测试、
  `parity_over_multibyte_paths` 和 `parity_sweep_over_real_corpus` 三条同时变红
  （ASCII 那两条仍绿，正是它们看不见这件事的证明）；
* 全量语料重做差分：231,123 行**与真 fzf 逐字节一致**；
* Rust 192 passed / 0 failed。

`KeystrokeBudgetTests` 这轮红（9.0ms vs 8ms 预算），但它在今天动 core **之前**
那轮就红过，而且当时是 18ms —— 一条跨在预算上的计时抖动测试，和这次改动无关。

### 真机验证

重新打包装进 `/Applications` 之后，用 `m4a` 做探针 —— 这条查询的新旧排序在**前
八行就不一样**，所以肉眼可验。先按 app 自己的 excludes（`Library`、`.Trash`、
`.cache`、`.npm`、`.cargo`、`node_modules`、`.git`、`Applications`）从那份 107 万
条的 dump 里还原它的语料，得到 **258,367 条 —— 和 app 自己报的索引条数一字不差**。

面板实拍的前四行和 `fzf --filter=m4a --scheme=path` 的前四行完全一致，而旧版本
排在第 1、2、4 位的 `11 朵.m4a`、`09 鼓楼.m4a`、`06 一生所愛.m4a` 现在落在第
11–13 行，正是新规则给它们的位置。

第 6、7 行两条 Zoom 录音和 fzf 互换：两条路径同为 86 字符、同分、同 pathname，
完全并列，次序由语料索引决定，而重建出来的文件顺序和 app 自己的索引顺序本就
不同。重建的产物，不是分歧。

### 附带修掉的一件事

用户报"选中后预览不见了"。预览没坏 —— `showPreview` 这个键在偏好里不存在，而
它默认是 `false`。第 28 步清理被测试污染的偏好时把两个域里的键都删了，连带把
预览列关掉了。按一次 Tab 就会写回去（`togglePreview` 持久化到偏好）。

## 第 31 步：默认精确匹配（以及 exact 词从来没有被评分）

用户搜 `nihao`，面板里出来一堆 `ni.*ha` 的路径：

> 看我搜索的是"nihao"可是出来的结果包含了"ni.*ha"的结果

第 30 步已经证明过这**不是 bug**：`fzf --filter=nihao` 在同一份语料上也匹配
174,504 条，两边逐字节一致。语义是对的，但对一个启动器来说答案是错的 —— 打三个
字母按回车的工具，给出十七万行不叫答案。用户定了方向：**默认精确，保留设置项和
快捷键，随时切换，当前查询立刻重算**。

### fzf 的 `'` 是模式开关，不是"精确匹配"

先在用户自己的语料上把 fzf 的语义量清楚（v0.70.0，107 万条）：

```
默认模式  nihao   : 174504      -e 模式   nihao   : 0
默认模式  'nihao  : 0           -e 模式   'nihao  : 174504
```

`'` 不是"按精确匹配这个词"，而是"按**另一种**模式匹配这个词"，`--exact` 决定
哪一种是"另一种"。所以 `Matching` 是解析期的参数，`classify` 的兜底和带引号的
那一支一起翻转。成对引号（`'foo bar'`）是这个项目自己加的，职责是**分组**，
两种模式下都保持精确 —— 否则精确模式下没有任何办法表达带空格的短语，而跨空格的
模糊匹配几乎匹配一切。

### parity 测试立刻抓到两件事：exact 词从来没有被评分

`app_path_parity.rs` 加一条 `-e` 的差分，第一次就红：

```
fzf ranks "search-core/src/index/test_parser.rs" above "indexer/src" for "s"
  keys of the first:  SortKeys { score: 0, ... trim_length: 36 }
  keys of the second: SortKeys { score: 0, ... trim_length: 11 }
```

两边都 0 分 —— 因为 `search.rs` 里 exact 词只是**硬过滤**（`literal::contains`，
在不在局内），注释写得明明白白"Only a fuzzy term scores"。而 fzf 的
`exactMatchNaive` 选出首字符边界分最好的那次出现，然后调**同一个
`calculateScore`**，和模糊匹配收尾用的是一个函数。全员 0 分意味着排序整体掉到
tiebreak 上，出来的顺序是 fzf 不认的。

所以新写 `core/src/fuzzy/exact.rs`：照抄 `exactMatchNaive`（不含
`boundaryCheck` 分支）+ 连续跨度上的 `calculateScore`。ASCII 快路用 memchr 找
候选起点（比 fzf 的逐字符回退快），非 ASCII 走字符路 —— 必须分开，因为开了
normalize 之后 ASCII 的 `e` 要能匹配 `é`，而字节扫描看不见这件事。同理，正向词的
`literal::contains` 过滤被**撤掉**了（`Literals::anchors_of`）：它折大小写但不折
重音，会把 fzf 认的 `résumé` 判出局。

修完第二条又红：

```
fzf ranks "indexer/src/search.rs" above "indexer/conf/search.rs" for "s"
```

fzf 把 `p.forward` **同样传给 exact 匹配器**。path scheme 的 tiebreak 链里有
`pathname`，`scan_direction` 因此是 Backward —— 要的是"尽量靠后的那次出现"，好让
`s` 算到文件名头上而不是目录头上。于是 `ExactMatcher` 也要 `.scanning()`：倒着枚举
出现位置，其余逻辑一字不变（同长度的出现，按 start 降序 == 按 end 降序）。

### 高亮也是同一处

`sk_engine_positions` 原来拿"第一个 Fuzzy **或 Exact** 词"去建**模糊**匹配器 ——
精确模式下每一行都会把散落的五个字母画上下划线。现在"哪个词评分、怎么评"只有一个
出处：`Scorer`（`Fuzzy` | `Exact`）由 `Scorer::compile` 生成，搜索用它打分、
FFI 用它取高亮位置，方向和选项都来自同一份 options。

### 收窄缓存

模式放进 `SearchOptions`，于是 options 的 stamp 一变，上一次查询的命中集就自动
作废 —— 同一段文字在新规则下不是同一个问题。`changing_the_matching_mode_drops_the_pool`
守着这件事：精确模式下 `tempe` 0 条，切模糊后必须是 100 条，而不是沿用那个空池。

然后 bench 的打字表露出一件更要紧的事：**新的默认模式下收窄从来没有生效过。**
`narrows()`（第 21 步）要求两侧的词都是 `TermKind::Fuzzy`，而精确模式下裸词是
`TermKind::Exact`，所以 `t`→`te`→`tes`→`test` 每一键都在 25.8 万条上重新跑，
"considered" 一次都不掉。第 21/24 步为打字做的事，被这个默认值静悄悄关掉了。

论证和模糊词是同一个，只换一个词：**精确词命中的条件是子串，而去掉子串的最后一个
字符剩下的还是子串**，所以在末尾长出来的词只会匹配得更少。条件由"两侧都是模糊"
改成"两侧同种，且同为模糊或精确"。两侧同种这一条不能省 —— `src` 作为子串不是
`src` 作为子序列的超集，所以 `src`→`'src`（换了模式）必须拒绝，`sr`→`sr*`（换成
glob）也一样。顺带 `'ab'`→`'abc'` 也从"明知道成立但不收"变成真的收，那条测试
（`extending_a_quoted_term_is_refused_although_it_would_be_sound`）跟着改掉。

效果比模糊模式好得多，因为子串的池小得多（同机，release，25.9 万条）：

```
             精确（新）                 模糊
keys       池      冷     收窄        池      冷     收窄
readme    1025   7.33 ms  0.33 ms   47160   7.85 ms  5.56 ms
read      5306   5.79 ms  0.75 ms       —   6.65 ms  6.61 ms
test     41701   8.76 ms  2.77 ms       —  10.79 ms 10.85 ms
fuzzy      147   6.17 ms  0.10 ms     362   2.10 ms  0.51 ms
```

`readme` 最后一键 0.33ms，模糊模式同一键 5.56ms；`test` 这一类在模糊模式下
一键都不收窄（每个前缀都匹配半个语料以上），精确模式下第三键就建了池。

### Swift 那一侧

`Preferences.exactMatch`（默认 true）→ `SearchEngine.exactMatch` →
`sk_engine_set_exact_match`。⌘E（keyCode 14，只认 Command 单独按）走
`PanelKey.exactToggle`。`setExactMatching` 写偏好、推引擎，然后直接调
`scheduleSearch()` —— 立刻重算就是这一行。设置窗口的勾和 ⌘E 走同一条
`UserDefaults.didChangeNotification` 回路，内层 guard 让第二次到达变成空操作，
所以不会搜两遍。输入框右边放了一枚 `exact` / `fuzzy` 的小药丸（也能点），
因为一个看不见状态的开关会让面板的答案显得随机。

### 验证

* `ExactQueryTests` 五条，真机索引（258,368 条）：
  `nihao` → 0 条，⌘E 之后 → 174,504 条，**和 `'nihao` 给的数字一模一样**
  （这是对照组：切模式必须等于显式要模糊）；
* `app_path_parity` 两种模式都绿，exact 那条的 pattern 里带 `'sr` `'ue`
  `'src` `'readme`，专门盯 `'` 的反转；
* Rust 206 passed / 0 failed。

性能也量了（`bench` 现在两种模式都跑，fzf 那一侧相应加 `-e`；真机 25.9 万条，
release）：

```
query        mode    seek              fzf
src          exact       5.50 ms           22.0 ms
src          fuzzy       8.34 ms          144.3 ms
test         exact       7.36 ms           47.8 ms
test         fuzzy      11.89 ms          185.5 ms
te           exact      10.80 ms          163.7 ms
s            exact      13.13 ms          199.5 ms
s            fuzzy      13.01 ms          193.7 ms
```

精确模式没有变慢 —— 多数查询反而更快（只用算一遍连续跨度，不用 DP），最坏的
单字符和模糊模式打平。注意精确词**用不上**上限剪枝（剪枝只对单个模糊词武装），
但它本来也不需要：memchr 找到第一个出现、碰到边界就停。

### 明知道还没对齐的地方

`^foo` 和 `foo$` 仍然只过滤不评分 —— fzf 的 `PrefixMatch` / `SuffixMatch` 也调
`calculateScore`，所以带锚点的查询排序和 fzf 还有差。这是这一步之前就有的分歧，
和模式无关，`exact.rs` 里的 `score_span` 正好是它要的那个函数。另外多词查询的
span：fzf 的 `buildResult` 取所有词的 minBegin / minEnd / maxEnd，这里取第一个词的
跨度；单词查询（也就是绝大多数）两者相同。

（两条都在第 32 步修掉了，连带还有三件当时没看见的事。）

## 第 32 步：锚点词从来没有被评分（以及 `^a$` 根本解析不出来）

第 31 步末尾记下的那条分歧：`^foo` / `foo$` 只过滤不评分。查下去发现它后面还跟着
三件事，都是同一类错误 —— 把 fzf 的某个决定读成了"显然如此"。

### `^a$` 解析成了一个永远匹配不上的词

我们的 `classify` 先剥 `^` 再剥 `$`，所以 `^abc$` 出来是 `Prefix("abc$")`：一个要求
路径以字面美元符开头的词。fzf 的顺序是反的（`pattern.go`）：先剥 `$`（带一个
`text != "$"` 的守卫），再剥 `^`，于是两个锚点同时在场时得到的是**第三种**形式 ——
`EqualMatch`，整条路径刚好就是这些字符。顺带两件小事也跟着对齐：单独一个 `$` 是
要找的字符而不是空后缀；`^` 和 `^$` 剥完什么都不剩，fzf 直接把词丢掉，我们的 parser
本来就丢空词。

### 锚点词只过滤不评分

fzf 的 `PrefixMatch` 和 `SuffixMatch` 末尾都调同一个 `calculateScore`，`EqualMatch`
则用闭式（每个字符都按空白边界算，`(scoreMatch + bonusBoundaryWhite) * len +
(bonusFirstCharMultiplier - 1) * bonusBoundaryWhite`）。实现上没有新东西：
`ExactMatcher` 加一个 `Anchor`（`Anywhere` / `Start` / `End` / `Whole`），把"找出现
位置"换成算术，评分仍旧走 `score_span`。空白裁剪也照抄 —— fzf 在锚定前裁掉行首行尾
空白，除非词自己要空白。

有一个结果值得写下来，因为它和直觉相反：**前缀词和全等词的分数对所有候选都相同**
（匹配的文本一样，前面那个字符的类也一样），所以单看分数它们是排序中性的。真正改变
顺序的是另外两件事：后缀词的分数（`rs$` 在 `src/index/rs` 里开在 `/` 后、在
`src/main.rs` 里开在 `.` 后，path scheme 给的分不一样 —— 实测 fzf 把前者排在前面，
我们现在也是），以及锚点词现在**提供了 span**，于是 `pathname` tiebreak 不再对
所有候选都是 `u16::MAX`。

### 多词的 span（第 31 步留下的另一条）

fzf 的 `buildResult` 取所有词 offset 的 `minBegin` / `minEnd` / `maxEnd`，我们取
第一个词的跨度。这一步一起修了（`search::span`）。变异测试证明它是真的：把 span 改
回"第一个词"，`^seek rs$` 立刻红 —— fzf 把 `seek/src/core.rs` 排在
`seek/conf/mod.rs` 前面，单独问它也坚持。

### `!` 把词变成 exact，模式说了不算

读 `pattern.go` 时顺手发现的：fzf 在读任何别的东西**之前**就把反向词设成
`termExact`，所以模糊模式下 `!ab` 也是子串排除。实测：

```
$ printf 'axb\nab\nzz\n' | fzf --filter='!ab'
axb
zz
$ printf 'axb\nab\nzz\n' | fzf --filter="!'ab"
zz
```

我们在模糊模式下把 `!ab` 当模糊词，排掉的比用户想的多得多。改成 fzf 的规则：裸的
反向词永远是 exact，`!'...` 永远是 fuzzy（`'` 要"另一种"，而 `!` 已经把它变成
exact 了，另一种只剩 fuzzy）。负向的 `^a$` 也得有人管，`Literals` 加了 `equals`。

### 收窄：前缀可以，后缀不行

前缀词能进 `narrows()` —— 以 `src` 开头的路径一定以 `sr` 开头。后缀词**不能**，
虽然它看起来是对称的：它的文本钉在远端，往里打一个字符会把已有的字符整体挪开，
`a$` → `ab$` 问的是另一对位置，`xab` 匹配长的那个而不匹配短的那个。全等词同理。

### 测试：树里加了两个没有扩展名的双胞胎

新的 parity 测试 `the_anchored_terms_match_fzf` 一开始就抓到了 `^confetti$`（集合
就不一样）。但排序那一半一开始**没有牙齿**：变异检查（把后缀词的分数改成 0）活了
下来，因为那棵生成的树里所有以 `rs` 结尾的路径前面都是 `.`，分数恒定。于是给树加了
`rs` 和 `md` 两个没有扩展名的文件，`rs$` 这才有了开在 `/` 后和开在 `.` 后两种情况 ——
同一个变异立刻红：

```
fzf ranks "search-core/src/index/rs" above "indexer/src/mod.rs" for "rs$"
```

### 顺带修掉的一件事

`limit * 2`（分块裁剪的阈值）在 `limit == usize::MAX` 时溢出，debug 下直接 panic。
`qrank` 用的就是 `usize::MAX`（它要和 fzf 的整个匹配集对比），所以这个例子在 debug
下一跑就炸。改成 `saturating_mul(2)`。

### 验证

* core **224 passed / 0 failed**；
* 11 个需要外部二进制的 parity 测试（3 个 fzf + 4 个 fd + 4 个 fzf_parity）全绿；
  锚点那条再用 `SEEK_PARITY_ROOT=~/projects` 对着真实目录树跑了一遍，也绿；
* 变异检查两条：后缀评分改 0 → 红，span 改回第一个词 → 红；
* app 140 条全绿（`KeystrokeBudgetTests` 的预算改成比例之后，见下）；
* release bench 前后对比在噪声内（`s` exact 13.07 → 11.63ms，`src` exact
  5.75 → 6.74ms，有快有慢），核心没有变慢。

### `KeystrokeBudgetTests` 的 8ms 预算，改成了比例

它量的是**面板在引擎之上多花的时间**：模型那条路 106–110ms，引擎单独跑 106ms，
差值 8.6–11.6ms，预算 8ms。这一步之前它就在这个带上飘（9.04ms、18ms 都红过，也
绿过）。差值两端用的是同一个核心，核心变慢会同时抬高两边、相减抵消，bench 也显示
核心没变 —— 不是回归，是预算本身的毛病。

毛病在于测试自己的注释说得很清楚（"independent of how fast that machine is"），
断言却是个绝对数。8ms 量的是机器：同一份代码，引擎那次跑 100ms 还是 70ms，决定了
它红还是绿。

所以先去量，再定数。同一台机器连跑十遍，面板那一份占引擎的 **1%–14%**
（1–15ms 对 110–124ms）；这个跨度就是测量本身 —— 两边都是 best-of-N，发现答案的
轮询有 1ms 粒度，调度器还有自己的想法。预算定在这条带最坏值的约两倍：

```swift
private let overheadShare: Double = 0.25
private let overheadFloor: TimeInterval = 0.008   // 引擎哪天快到不值得评价时的地板
```

松是故意的。这条测试要抓的是 sleep、多一次 hop、把整张结果表深比较一遍这类东西，
在一个约 100ms 的搜索旁边它们值几十毫秒，离这条带很远，而不是刚好越过边缘。
变异检查确认它还有牙齿：在 publish 路径里塞一个 30ms 的 `Thread.sleep`，差值变成
54ms（流式会 publish 不止一次），预算 27ms，红。

回头看，之前那次 18ms（占 16%）多半也是噪声而不是真回归。

## 第 33 步：菜单栏、界面精修，以及一个从来没被画出来的 placeholder

这一步是四件事：菜单栏入口、界面精修、查 bug、改名。前三件做完才发现第三件
其实藏在第二件里面。

### 没有入口的应用

`LSUIElement` 是 `true`，`applicationDidFinishLaunching` 里又 `setActivationPolicy(.accessory)`
—— Dock 图标早就没有了，这件事不用做。但没有 Dock 图标又没有菜单栏，这个应用
就一个门：热键。忘了热键的人连"它在不在跑"都看不出来。

所以加 `NSStatusItem`：一个 template 的放大镜（template 才会跟着菜单栏黑白翻转，
彩色图标会在两种外观里各消失一次），菜单里四行 —— 显示面板（标题里带当前热键）、
索引状态、设置、退出。

两个细节值得写下来：

- **热键写在标题里，不做 keyEquivalent。** 做成 key equivalent 就是给全局热键
  已经在应答的那个组合再加一个处理者，两个会抢一个已经开着的面板。
- **索引那一行在菜单打开时才填。** 索引是会长的；启动时写死一个数字，之后
  一整个会话都是快照里的旧值。`menuNeedsUpdate` 里顺便把热键也重写一遍，
  因为它能在设置里改。

图标本身也带状态：`appearsDisabled` 跟着 `model.$isIndexing` 走，索引没建完
放大镜是灰的。变异检查确认这条有牙齿：把 sink 里的赋值换成 `_ = indexing`，
测试红（`false` 不等于 `true`）。

### 界面精修（克制的那种）

先把面板渲染成 PNG 看（`--snapshot=`，第 29 步加的），再动手。看出来的：

- **选中行的对比度比没选中的行还低。** 选中行的底色是 accent 的 22%，匹配到的
  字又正是 accent —— 全panel 里最该看清的那一行，恰恰是最看不清匹配在哪的那一行。
  底色降到 15%。
- **`exact` 芯片长得像个禁用控件。** 灰字、灰底、外面还套着灰色的输入框 well，
  三层几乎同一个灰。它既不是禁用的也不是装饰：它是决定查询是什么意思的那个开关。
  改成 accent 字 + accent 13% 底。
- **footer 的键帽没有边。** 0.06 的底在 10.5pt 字下面基本不存在，`↑` 读起来
  像 footer 那句话里的又一个词。底提到 0.08，加一条 0.5pt 的描边。

顺手把散在视图里的几个 opacity 收进 `Theme`（`selection` / `well` / `wellBorder`
/ `keyFill`），这样"精修"是可以 diff 的。

### 查 bug：placeholder 从来没有被画出来

看快照的时候发现输入框是空的 —— 不是没有文字，是连 placeholder 都没有。
一个光标，一个灰盒子，再没别的。

原因和第 31 步的 exact 词是同一类：**属性一直是对的，对的东西从来没有生效。**
`TextField("Search files and folders", ...)` 一直带着它的 placeholder，而 AppKit
在字段进入编辑状态的那一刻就不再画 placeholder 了 —— 面板打开就把焦点给输入框，
这正是启动器的全部意义，所以这行字一次都没上过屏幕。

修法是自己画：`query.isEmpty` 时在 `TextField` 上叠一层 `Text`。

测试按墨量来写，不按属性来写 —— 属性本来就是对的。把面板渲染出来，数输入框
那一条里的墨：有提示 3444 个像素，空字段 0 个。阈值 1000 / 100，两边都离得很远。
测试里显式 `makeFirstResponder(field)` 并断言 `currentEditor() != nil`，否则
量到的是一张用户永远看不到的图（没有焦点时 AppKit 自己会画 placeholder，
测试就没有牙齿了）。

写这条测试时踩了一个坑，值得记：第一版的 scale 是 `pixelsHigh / size.height`
算的，量出来 196 像素，怎么改都不动。因为面板的高度是跟着内容走的 —— 给它
460pt，它只用了 196pt，于是 scale 成了两个毫不相干的数之比（0.85，真值是 2），
整个取样框落在了别处。改成从宽度算（宽度是 `Theme.panelWidth`，固定的）。

### 改名：fuzzylite → Seek

全量：crate（`seek-core` / `seek_core`）、FFI 前缀（`fl_` → `sk_`）、Swift 模块
与测试目标、bundle id（`dev.seek.app`）、快照文件（`index.skidx`）、快照魔数
（`b"FUZZYLIT"` → `b"SEEKIDX1"`，这个不能靠全局替换，它少一个 E）、文档。

### 验证

- core 133 passed / 0 failed；11 个 `--ignored` 的外部二进制对拍（3 fzf + 4 fd
  + 4 parity）全绿
- app 147 passed / 0 failed（原 140，新增 5 条菜单栏 + 2 条 placeholder）
- `./bundle.sh` 出包、`--smoke-test` 绿（`top hit matches at [0..8]`）
- 前后两张 `--snapshot=` 对照看过


## 第 34 步：通用二进制（arm64 + x86_64，macOS 14 起）

出的包一直只有 arm64 一个 slice。改掉：`bundle.sh` 现在给两个 target 各编一次
核心，用 `lipo` 把两个静态库合成一个通用归档，再交给一次
`swift build --arch arm64 --arch x86_64`。`build.sh` 不动 —— 那是每次改完都要等的
那一个，还是只编本机。

最后一段是**问二进制它自己是什么**：缺一个 slice、或者某个 slice 的最低版本不对，
都会让构建失败。这两种情况都长得和构建成功一模一样，直到包被拷到另一种 Mac 上。
最低版本要逐 slice 查（`otool -arch`），因为 `MACOSX_DEPLOYMENT_TARGET` 只保证得了
本机编的那一个。

验证不止看元数据：x86_64 那个 slice 用 `arch -x86_64` 真跑了一遍 smoke test，绿。

需要一次性装好 Intel target：`rustup target add x86_64-apple-darwin`。


## 第 35 步：追那两条间歇性失败的测试（三个，都在测试自己身上）

第 34 步之后有一次跑出 `148 tests, with 2 failures`，名字当时没抓到。重跑五次全
绿，所以先造复现条件：把仓库拷一份、用自己的 build 目录，和真树同时跑 —— 机器
一满，第一次就复现了，名字也拿到了。

**两条阈值失败（就是当初那个 2）**：

- `KeystrokeBudgetTests` 的绝对上限是 150ms，量出来 131ms，满载 152ms。这个数
  里**几乎全是引擎**：引擎自己 124–140ms，面板只加 6–11ms（有一次 −6ms，就是噪
  声）。它的注释写的是"一个宽松的上限，用来兜住引擎本身的退化"，而 13% 的余量
  并不宽松 —— 它管的不是面板，是机器。有牙的是下面那条（面板开销 vs 引擎），
  因为两边一起慢，比值不动。上限 → 300ms，注释里记下量到的数。
- `PanelDrawsResultsTests` 要求 `rows > emptyRows * 3`，`emptyRows` 是 5，
  `rows` 平时 16、满载 15 —— 这条杠离它自己的量只有**一个文字带**。行数是从位图
  上数出来的带数，列表满屏时最后一行被视口切掉，所以本来就会差一。改成
  `* 2`（杠 10，量 16）；"一屏"由下面那条绝对值 `rows > 8` 钉住。

**第三条，8 条失败，是另一个毛病**：冷跑第三次 `PanelRetypingTests` 把打进去的
`human` 读回来成了 `Shuemuarn`。泄漏的监视器日志把真相印出来了 ——
`s·h·u·e·m·u`，正是那段残留。原因：`press` 用 `matching: .any` 抽**进程级**事件
队列并把抽到的全发出去，于是别的测试留在队列上的键被送进了这个面板。顺带看到每
个键被打印三遍：有三个泄漏的监视器还活着。

修的过程错了两次，都是自己量出来的：

1. 先按 `queued === event` 过滤 —— 全军覆没（`query=""`）。`postEvent` 交出去的
   事件**不是**抽回来的那个对象，AppKit 会拷一份，按身份认不出来。
2. 改成"先清队列、再 post"，并在 `!panel.isKeyWindow` 时补发给面板 ——
   变成 `hhuummaann`。"没有 key window 时 `NSApp.sendEvent` 送不到"这个前提
   在单跑时成立（`query=""`），在全量跑时不成立，所以同一个键发了两遍。
   `panel.isKeyWindow` 预测不了 `sendEvent` 到底送到没送到。

最后按最简的那条走：**直接 `panel.sendEvent(event)`**，和其余打字测试一样。走
`NSApp` 这条队列路对这个测试本来就什么都没多测 —— 应用自己的键监视器是 delegate
装的，而这个测试自己搭面板，监视器从来不在这条路上（全量跑时能看到键，那是泄漏
的监视器，是意外不是覆盖；真正覆盖它的是 `AppDelegateIntegrationTests`）。这个
测试独有的东西是**键的顺序**：一个词、整词删掉、再换一个词，也就是当初报上来的
那个序列。类头的注释跟着改对了。

牙是量过的：上面两次改错时，断言都大声失败了（`""` 和 `hhuummaann`），不是默默
通过。

验证：全量 148 条连跑两次 0 失败 0 警告（且不再有 skip），再**满载**（所有核心
跑死循环）连跑两次仍 0 失败。耗时 183–193s → 172s。三个改动全在测试里，产品代码
没动一行 —— 这三条都不是产品的毛病。

顺带量到一件事，没有修：每条测试都漏掉自己的 `AppModel`，每个都在扫 258,373 条，
而且 `SearchEngine.defaultSnapshotPath` 是**用户真实的**
`~/Library/Application Support/Seek/index.skidx`，每次 `AppModel()` 都读它、扫完
无条件写它。所以同时跑两份测试（或一边用 app 一边跑测试）会互相把索引盖掉 ——
我自己撞上过一次，`indexedCount` 变成 6。这也是整套测试要三分多钟的原因。

## 第 36 步：空查询不再说话，以及出 dmg

**面板**：空查询下那块"Type to search / Fuzzy matching over the whole path…"去
掉了。它是唯一一个**每次召出面板都会出现**的空状态 —— 而不是在有什么需要解释的
时候才出现 —— 并且第 33 步给搜索框补上 placeholder 之后，它说的话和输入框里已经
写着的那句重复了。现在空查询什么都不画，面板就是一条搜索框。

另外三个空状态留着，它们各自回答"这个列表为什么是空的"：查询读不懂（带原因）、
索引还在建、没有匹配。顺序也跟着改对了 —— 原来 `query.isEmpty` 排在最前面，所以
空查询且正在建索引时显示的是"Type to search"而不是"Building the index"。

padding 挪进了有内容的那一支：空容器照样吃自己的 padding，包在外面的话，空查询会
在原地留下 52pt 的空白。

**dmg**：`bundle.sh` 最后多出一张盘，`build/Seek-<版本>.dmg`，里面是 app 和一个
指向 `/Applications` 的符号链接。光发一个 .app 过去，签名经不起大多数传输方式；
一张盘寄出去是什么样，收到就是什么样。

盘也要问它自己装了什么（和第 34 步问二进制一样的理由）：挂上去、确认里面的可执行
文件在、逐 slice 查架构、`codesign --verify`，然后卸载。一张用半成品装出来的盘，
挂上去看着也是对的。

验证：全量 148 条 0 失败 0 警告；盘里两个 slice 都是 14.0、签名有效、
`Fuzzy matching over the whole path` 这句在二进制里已经找不到，另外三个状态还在。

**还没做**：签名是 ad-hoc 的，没有 Developer ID、没有公证，所以发给别人时
Gatekeeper 会拦（"无法验证开发者"），对方要右键打开或者自己去掉隔离属性。

## 第 37 步：半满的索引不再假装自己是完整的

扫描有 60 秒上限，撞上了索引就是半满的。引擎当时知道这件事（`sk_engine_scan_complete`），
面板也会在footer 里写 `· scan cut short`。问题是**这个知道法只活在内存里**：

- `persist.rs` 的 `Snapshot` 里没有任何一位记"这次扫完整了没有"；
- `AppModel.startIndexing` 是**无条件**存快照的，`complete` 只 publish 不落盘；
- `restoreIndex()` 只设 `indexedCount`，**没碰 `scanComplete`**，而它的默认值是
  `true`（AppModel.swift:83）。

所以从半满快照恢复回来之后，那句 `· scan cut short` 就没了 —— 第 883 行是
`scanComplete ? count : "\(count) · scan cut short"`。应用声称自己是完整的。
`ffi.rs:134` 那条注释写的正是"把半满索引报成完整的会误导"，只是这个道理没有
跟着数据走到盘上。

改掉：`Snapshot` 多一位 `complete`，`persist::save` 多收一个参数，
`sk_engine_save_snapshot` 把 `inner.scan_complete` 写进去，
`sk_engine_load_snapshot` 恢复时把它读回 `inner.scan_complete`，
`restoreIndex()` 再把引擎的值抄给模型。

格式版本 1 → 2，magic `SEEKIDX1` → `SEEKIDX2`。**不做兼容**：旧快照直接被拒，
升上来之后第一次启动会整扫一遍。

牙量过了：把 `inner.scan_complete = snapshot.complete` 这一行去掉，
`a_restored_partial_index_still_knows_it_is_partial` 立刻红，还原后绿。

**范围说清楚**：这一步修的是"半满的索引不再谎称完整"，不是"扫描不再超时"。
60 秒上限还在，索引仍然要等某一次扫描真的跑完才会补齐 —— 只是现在它会一直
老实说自己是半满的。

Swift 那一行（`restoreIndex` 读 `engine.scanComplete`）没有直接的测试：
`restoreIndex` 是 private，而造一份半满快照要走 core 的 API，Swift 侧没有对应
的 FFI。我没有为了测试往产品里加接口。真相由 core 的两条测试钉住。

## 六、下一步的优先级（按 `fd-and-fzf-are-the-core` 规则）

1. ~~**确认面板真的显示结果**~~ —— 第 9 步已修复并有测试守着
2. ~~**对齐 fzf 的 path scheme**~~ —— 第 10 步已对齐，差分测试守着
3. ~~**性能优化**~~ —— 第 11 步已进预算（最慢 43 ms）
4. ~~**UI 精修**~~ —— 第 12 步已做（四个缺陷 + 自适应高度）
5. ~~**glob / regex**~~ —— 第 13 步已做，顺带修了三个潜伏的查询 bug
6. ~~**应用图标**~~ —— 第 14 步已做（画成代码，小尺寸另有简化版）
7. ~~**footer 报匹配数**~~ —— 第 15 步已做（搜索本身没问题，是 footer 在说谎）
8. ~~**砍掉外壳延迟**~~ —— 第 16 步已做（43ms → 26ms，外壳开销 2ms）
9. ~~**放开 200 行上限**~~ —— 第 17 步已做（按页取，顺带修掉静默丢结果的 bug）
10. ~~**空转的 CPU 和内存增长**~~ —— 第 18 步已做（34.8% → 4.0%，365MB 增长
    → 211MB 平稳；顺带修掉"删除从未生效"和索引无界增长）
11. ~~**剩下的 4%**~~ —— 第 19 步已做：一个不响应的目录把 `opendir()` 堵住
    54 分钟，走盘不返回，其余 walker 按 `ignore` 的设计每 1ms 空转查退出条件。
    走盘不再跨文件系统，扫描带 deadline（0.0–0.4% CPU，10 秒出索引）
12. ~~**内存 285MB 偏高**~~ —— 第 20 步已做：不是数据结构胖（活数据 40.6MB），是索引
    那一瞬间约 60 万个短命小分配的峰值。修了三处峰值加上两代 arena，
    app RSS 285 → 250MB（剩下约 140MB 是 AppKit/SwiftUI 的底子）
13. ~~**增量收窄**~~ —— 第 21 步已做，但**没解决它本来要解决的问题**：选择性查询
    5 倍（1.5ms → 0.3ms，人感觉不到，外壳还有 2ms），而发涩的 `test`／`src`
    一键都没生效。模型是 `1.4ms + 200ns × 命中数`，收窄只能削掉那 1.4ms。
    第 31 步把它扩到精确词之后反而成了默认模式的主力：子串的池小得多，
    `test` 第三键就建池（8.8 → 2.8ms），`readme` 最后一键 0.33ms
14. ~~**去掉窗口按钮**~~ —— styleMask 里去掉 `.closable`（那条注释已过期，Escape 走
    的是 `hidePanel()`）；`.titled` 必须留着，它是面板能成为 key window 的原因
15. ~~**慢查询的真正杠杆**~~ —— 第 22 步已做，两条路都走了：DP 内层循环改行切片
    （边界检查消失、左邻走寄存器、不再清零整个矩阵），以及进不了前 N 名的命中项
    不打分（上限论证 + 共享计数器 + 四条守卫）。`test` 30.4 → 11.0ms（2.8×）、
    `src` 18.4 → 7.3ms、`index` 11.8 → 6.8ms。顺带发现代码体积会让单字符查询
    慢 15%，拆 `score_matrix` 修掉
16. ~~**`s` 这一类的 14ms**~~ —— 第 25 步量清楚（匹配 3.2 ＋ 排序键 2.4 ＋ collect
    0.6 ＋ **合并 4.8**），第 26 步修掉：下限从"每块算一次"改成"push 循环里随时
    更新"，`s` 12.0 → 9.4ms、`t` 12.4 → 9.7、`e` 12.5 → 9.8。**现在所有键都在
    10ms 附近**。m=1 用边界剪枝量过了，`s` 只能剪 16%，净亏，否掉。结构性的那条
    （每个 worker 自己维护 top-N，去掉 8.2MB 的 collect 和巨型选择）估计再省
    4–5ms，但要重写搜索的数据流并和分块发布对齐 —— 现在没有必要了
17. ~~**`te` 这一类的 22ms**~~ —— 第 23 步量清楚（坎在 m=1 → m=2 的一次性台阶，
    88.5% 的命中真的含连续子串），第 24 步修掉：判据换成"在分隔符后含连续子串"，
    剪掉比例 11% → 72%，`te` 20.6 → 10.3ms。重证时发现一类必须排除的查询
    （模式自带边界，如 `/b`，无边界时也能到上限 − 2）
18. ~~**预览面板**~~ —— 第 27 步已做：Tab 切换、QuickLook 全类型、面板 720 ↔ 1080
    居中变宽。放置算术抽成纯函数 `PanelPlacement`（六条测试），目标防抖 120ms
    （只加载停下的那一行），设置窗口的三个灯一并去掉。**遗留**：这台机器的
    QuickLook 没有 Markdown 文本生成器，`.md` 预览出来是类型图标 —— 同一步里改掉了：
    文本自己读自己画（先类型后字节、头部 64KB / 400 行），其余仍交给 QuickLook
19. ~~**流式出结果**~~ —— 第 23 步已做（三层 + 32 条测试；分块代价 ±1.5ms，
    第一页 1.6–4.4ms，被取代的搜索放弃后 `te` 省 15.2ms）
20. ~~**锚点词按 fzf 评分**~~ —— 第 32 步已做，顺带修掉 `^a$`（本来解析成一个
    匹配不上的前缀词）、多词 span（取所有词的 min/max 而不是第一个词）、
    `!` 的词种（fzf 里反向词永远是 exact）
21. ~~**菜单栏入口与界面精修**~~ —— 第 33 步已做（`NSStatusItem` + 菜单、
    选中行对比度、`exact` 芯片、键帽描边），顺带修掉 placeholder 从来没有被画
    出来（焦点一进字段 AppKit 就不画它了，而面板总是带焦点打开）
22. ~~**半满索引谎称完整**~~ —— 第 37 步已做：快照记下扫描的完整性，恢复时
    尊重它。原来列表里写的"直到重启"是乐观的 —— 重启恢复的就是那份半满快照。
    **仍然留着的**：60 秒扫描上限本身，索引还是要等某次扫描跑完才补齐
23. **测试之间共用状态** —— 第 35 步量到，没有修：每条测试漏掉自己的
    `AppModel`（各扫 258,373 条），并且共用用户真实的 `index.skidx`，
    所以两份测试同时跑会互相盖掉索引；也是整套跑三分多钟的原因
24. **剩下的**：惰性 hydration、`content:` 操作符、
    最近打开过的文件、首启动权限引导、NSLog 进不了统一日志、
    apply 路径不遵守 `.gitignore`、
    扫描 60 秒上限（索引要等某次扫描跑完才补齐）、
    文件夹的预览是一个空盒子（QuickLook 对目录没有内容，要自己列出目录条目）

> 顺序不能颠倒：一个慢的搜索比丑的面板更糟，而一个丑的面板只有在搜索不慢的时候
> 才是可以接受的问题。
