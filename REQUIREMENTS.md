# seek — 需求文档

> 一个把 `fd` 的遍历和 `fzf` 的排序装进 macOS 原生面板的启动器。
>
> **核心原则：fd 和 fzf 是核心，UI 只是外壳。**

---

## 1. 为什么要做这个 App

### 1.1 Spotlight 的三个问题

macOS 自带的 Spotlight 是系统级搜索，但它的设计和开发者的实际工作流是脱节的：

| 问题 | 具体表现 |
| --- | --- |
| **范围不可控** | Spotlight 索引整块磁盘。开发者根本不想在 `node_modules`、`.git`、构建产物、虚拟环境里搜文件。真正需要搜的往往只有 `~/projects`、`~/Documents`、`~/Downloads` 这几个目录。 |
| **排序不可预期** | Spotlight 的排序有自己的启发式规则（recency、app 匹配度、metadata 权重）。它**不是**子序列匹配，输的是"相关性"而不是"你输入的那几个字符"。开发者习惯了 fzf 的排序，反而不适应 Spotlight。 |
| **配置成本高** | 想让 Spotlight 只搜某几个目录，要进系统设置一层层点。排除规则更难配。而真正的需求往往是"我要搜我的代码目录，忽略所有依赖目录"。 |

### 1.2 开发者已有的、更好的工具

开发者**每天都在终端里**用两个工具：

- **`fd`** —— 比 `find` 快的递归文件查找。默认遵守 `.gitignore`、`.ignore`、`.fdignore`，默认跳过隐藏文件，支持 `--exclude` 排除模式。
- **`fzf`** —— 模糊搜索器。它的 `FuzzyMatchV2` 算法和它的 tiebreak 排序链（score → length → chunk → pathname → begin → end）**非常好**，是整个开发者社区公认的"最好用的排序"。

问题是：**这两个工具都在终端里**。

开发者的工作流是：想到一个文件名 → 切到终端 → `cd` 到某个目录 → `fd xxx | fzf` → 看到结果 → 切回来。这中间有 3–4 次窗口切换、1 次 `cd`、1 次管道命令。对一个每天要做几十次的事情来说，这个摩擦是实实在在的。

**seek 就是把这个已经存在的、已经被验证过的工作流，压缩成"按一个热键 + 打字"。**

### 1.3 这个 App 的定位

不是"又一个搜索框"，而是：

> **一个专门给终端用户用的、行为和 `fd | fzf` 完全一致的 macOS 启动器。**

它**不是**要重新发明搜索算法，而是要把 `fd` 和 `fzf` 的行为**精确移植**到一个原生 macOS 面板里，并且做得比在终端里更顺（不用切窗口）。

### 1.4 成功标准

> 敲一个键，正确的文件出现在列表顶部；再敲一个键就打开它。

具体化：

- 按下热键到面板出现：**< 100ms**（含热键响应）
- 按下最后一个字符到结果出现：**< 50ms**（在 25 万文件的索引上）
- 排序结果：与真实 `fzf --filter` **逐位一致**
- 遍历范围：与真实 `fd` **完全一致**

---

## 2. 这个 App 的功能是什么

### 2.1 核心功能

#### F1 — 全局热键唤起面板

- 默认 `Ctrl+Space`（和 Alfred、Raycast 一致，避开 Spotlight 的 `Cmd+Space`）
- **用户可自定义**任意组合键，在设置里"按一下就录"
- 面板悬浮在所有窗口之上
- 再按一次热键 / `Esc` / 点击别处 → 关闭
- 闲置超时自动隐藏（可配置，默认 30 秒）
- 关闭后焦点回到原来的 App

#### F2 — 模糊搜索文件与文件夹

- 输入即搜，无需回车
- 搜索范围 = 用户配置的文件夹（**只搜用户指定的**）
- 匹配的是**整条相对路径**，不是只有文件名 —— 所以输入目录名能找到里面的文件
- 同时返回文件和文件夹

#### F3 — fzf 的排序（**核心契约**）

排序严格复刻 fzf 的 tiebreak 链。**链条由 scheme 决定**，不是一个固定的列表 ——
这正是 fzf 自己的做法：

```
--scheme=default →  score → length
--scheme=path    →  score → pathname → length
```

app 用的是 path scheme，所以链条是 `score → pathname → length`。
（fzf 还会在每条链末尾隐式追加 `index`，即输入顺序；app 靠稳定排序
 + corpus 顺序得到同样的效果。）

链条还决定**匹配器的扫描方向**，这一点容易漏：`pathname` 和 `end` 要求
匹配尽量靠后，`begin` 要求尽量靠前。同分的多个匹配位置里报哪一个，直接
决定了 `pathname` 把这一条算成"目录命中"还是"文件名命中"。见
`core/src/fuzzy/rank.rs` 的 `scan_direction`。

匹配分数用 fzf 的 `FuzzyMatchV2` 算法：动态规划评分矩阵 + 字符分类 + bonus 矩阵（词边界、分隔符、camelCase hump、连续匹配）+ 拉丁重音折叠。

**如果排序和 fzf 不一致，那是 bug，不是设计选择。**

#### F4 — fd 的遍历（**核心契约**）

- 默认遵守 `.gitignore`、`.ignore`、`.fdignore`
- 默认跳过隐藏文件
- 排除模式用 fd 的 `!pattern` 约定
- 显示的路径是**相对索引根目录**的路径 —— 和 `fd` 给定根目录时打印的东西一样

#### F5 — fzf 的查询语法

| 语法 | 含义 |
| --- | --- |
| `foo` | 模糊匹配（子序列） |
| `'foo'` | 精确子串匹配 |
| `^foo` | 前缀匹配 |
| `foo$` | 后缀匹配 |
| `!foo` | 排除匹配 `foo` 的结果 |
| `foo bar` | 两个词都必须匹配 |

#### F6 — 实时索引（增量更新）

- FSEvents 监视器监听索引目录
- 启动后新建/修改/删除的文件立刻能被搜到，**不需要重启**
- 索引快照落盘，下次启动是热的（秒开）

#### F7 — 键盘优先的操作

| 键 | 动作 |
| --- | --- |
| `Ctrl+Space` | 显示 / 隐藏面板（可改） |
| `↑` `↓` | 移动选中项 |
| `Home` `End` | 跳到第一个 / 最后一个 |
| `Return` | 打开 |
| `Cmd+Opt+R` | 在 Finder 中显示 |
| `Cmd+Shift+C` | 复制完整路径 |
| `Cmd+,` | 打开设置 |
| `Cmd+Q` | 退出 |
| `Esc` | 关闭面板 |

**鼠标是可选的。** 任何主要功能都不该需要指针。

#### F8 — 设置

| 设置项 | 说明 |
| --- | --- |
| 快捷键 | 点"Change…"然后直接按组合键。默认 `Ctrl+Space` |
| 索引文件夹 | 一行一个，`~` 会展开。有"Add Folder…"按钮 |
| 排除模式 | 一行一个 glob，行为和 `fd --exclude` 一致 |
| 匹配选项 | 大小写敏感、忽略重音（é 匹配 e）、评分方案 |
| 隐藏时机 | 闲置多久自动隐藏（Never / 15s / 30s / 1min / 5min） |

设置窗口还有"立即重启"和"退出"按钮 —— 因为这个 App **没有 Dock 图标、没有菜单栏**，不给按钮的话用户根本没法重启它。

#### F9 — 无干扰的存在形式

- `LSUIElement = true`：无 Dock 图标、无菜单栏
- 只是一个悬浮面板，用完即走
- 焦点永远在搜索框，不用鼠标点

### 2.2 明确不做的事

| 不做 | 原因 |
| --- | --- |
| `content:` 内容搜索 | 还没实现。设置里显示为**禁用**并标注 "not available yet"，而不是放一个静默无效的开关 |
| 全文索引 | 超出"替代 fd + fzf"的定位 |
| 重新发明排序 | fzf 的排序就是契约 |
| 自动改变搜索范围 | 用户配了什么就搜什么，不偷偷加范围 |
| 剪贴板历史、计算器、快捷启动 | 那是 Alfred/Raycast 的定位，不是这个 App 的 |

---

## 3. 主要优点

### 3.1 相比 Spotlight

| | Spotlight | seek |
| --- | --- | --- |
| 搜索范围 | 整块磁盘，不可控 | **只有你指定的文件夹** |
| 排序 | 自己的启发式规则 | **fzf 的排序，逐位一致** |
| 排除规则 | 难配 | 和 `fd --exclude` 一样简单 |
| 匹配语义 | 相关性 | **子序列匹配**，输入什么都能搜到 |
| 配置 | 系统设置里几层菜单 | App 自己的设置，`~` 展开，一行一个 |
| 结果高亮 | 弱 | **fzf 式高亮**，匹配字符加重 + 着色 |

### 3.2 相比终端里的 `fd | fzf`

| | 终端 | seek |
| --- | --- | --- |
| 切换成本 | 3–4 次窗口切换 + `cd` + 管道 | **一次热键** |
| 排序 | 一样 | 一样 |
| 索引 | 每次现扫 | **常驻内存 + 实时更新** |
| 结果预览 | 靠 fzf 的 preview | 面板内直接看路径 |
| 打开文件 | 还要切回 GUI | `Return` 直接开 |

### 3.3 相比 Alfred / Raycast

| | Alfred / Raycast | seek |
| --- | --- | --- |
| 体积 | 几十 MB | **Rust 静态库 + SwiftUI，几 MB** |
| 内存占用 | 较高 | **低**（紧凑索引 + 零分配匹配器） |
| 搜索语义 | 通用搜索 | **严格是 fd + fzf 的语义** |
| 配置 | 功能很多 | **只做搜索这一件事** |

### 3.4 性能

实测（`~/`，233,505 个文件 / 258,376 条索引）：

| 查询 | seek | fzf | 倍数 |
| --- | --- | --- | --- |
| `seek` | 1.6 ms | 2.7 ms | 1.7× |
| `index` | 12.6 ms | 44 ms | 3.5× |
| `s` | 16.9 ms | 189 ms | 11.2× |
| `cargo` | 17.0 ms | 54 ms | 3.2× |
| `src` | 29.3 ms | 144 ms | 4.9× |
| `test` | 43.4 ms | 186 ms | 4.3× |

（fzf 的数字还**包含进程启动开销**。）

内存：常驻 60.8 MB（索引 18.4 MB + corpus 42.4 MB）。

**为什么更快**：

1. **常驻索引** —— fzf 每次要 spawn 进程、扫目录、读管道；seek 什么都不做，直接在内存里的 25 万条路径上匹配。
2. **零分配匹配器** —— DP 矩阵用可复用的 scratch buffer，每个 worker 一个，
   不为每条候选分配内存（这句话一度是假的，见 DEVELOPMENT.md 第 10 步）。
3. **扁平可切片** —— corpus 是连续的 `Vec`，Rayon 可以直接切给多核。
4. **排序键预先算好** —— 排序时只比整数，不比字符串；不依赖查询的 key
   （如 `trim_length`）在建 corpus 时就算好，fzf 也是这么缓存的。
5. **只排要显示的那部分** —— 面板只显示 500 行，就只排 500 行。
   这要求比较是全序，所以 fzf 隐式的 `index` tiebreak 在这里是显式写出来的。
6. **ASCII 快路径** —— `fold` 不走 `char::to_lowercase`（它每字符构造一个迭代器），
   prefilter 用 `memchr` 向量化。
7. **批量 FFI** —— 一次加锁解析全部结果的路径和匹配位置，不是每条结果调一次。

---

## 4. 借鉴了哪些 App / 项目

### 4.1 `fd`（https://github.com/sharkdp/fd）

**借鉴**：整个遍历层。

- 用 `ignore` crate 做并行遍历（fd 自己也是用它）
- `.gitignore` / `.ignore` / `.fdignore` 的遵守方式
- 隐藏文件的默认跳过
- 排除模式用 `!pattern` 约定（fd 的惯例）
- **显示相对根目录的路径**（fd 给定根目录时的打印方式）
- Apache-2.0 许可，可商用

**借鉴方式**：**行为对齐，不是代码搬运。** 用差分测试对着真实 `fd` 二进制验证：
找到的路径集合、隐藏文件处理、`.gitignore`/`.fdignore`、排除模式，4 项全部一致。

### 4.2 `fzf`（https://github.com/junegunn/fzf）

**借鉴**：整个匹配与排序层。

- `FuzzyMatchV2` 算法：Smith-Waterman 式动态规划评分矩阵、字符分类、bonus 矩阵（词边界 / 分隔符 / camelCase hump / 连续匹配）、拉丁重音折叠
- **tiebreak 排序链**：score → length → chunk → pathname → begin → end
- 查询语法：`'exact'` `^prefix` `suffix$` `!exclude` 空格分隔多词
- 匹配整条路径而非只有文件名

**借鉴方式**：**自己移植 + 差分测试验证。** 1,822 行的 Rust 移植
（`core/src/fuzzy/`），对着真实 `fzf --filter` 二进制做差分测试，
比对匹配集合、分数和完整排序顺序，全部一致。

**为什么不用 `nucleo`（Helix 同款）**：nucleo 是"类 fzf"，不是 fzf。
用了它就过不了 fzf 差分测试，而 **fzf 的排序就是契约**。

### 4.3 Cardinal（https://github.com/cardisoft/cardinal）

**借鉴**：两个设计思路，不是代码。

1. **索引结构** —— slab 分配 + 字符串驻留（interning）的紧凑索引。
   本项目实现为 `Index`（`parent: u32` + `name: u32` interned + `Vec<SlabIndex>` 稠密数组）。
2. **版本化取消机制** —— 搜索可被取消/失效而不影响已发布的结果。
   本项目实现为单调递增的 `generation` 计数器：过期结果直接丢弃，
   **不取消进行中的搜索**（取消会连带把结果一起扔掉，见 DEVELOPMENT.md 第 5 步）。

### 4.4 Alfred / Raycast

**借鉴**：**交互形态**，不是代码。

- `Ctrl+Space` 热键唤起（避开 Spotlight 的 `Cmd+Space`）
- 悬浮居中面板，而不是普通窗口
- 闲置自动隐藏
- 键盘优先：每个动作都有快捷键
- 无 Dock 图标、无菜单栏（`LSUIElement`）
- 设置里有"录一下就生效"的快捷键捕获

### 4.5 Helix

**借鉴**：**架构模式**（通过 `nucleo` 间接认识）。

- 后台线程池做匹配
- 快照式发布结果
- 主线程只负责渲染

本项目用 Swift 结构化并发 + Rust Rayon 实现了同样的模式。

---

## 5. 技术架构

### 5.1 选型

| 层 | 选型 | 理由 |
| --- | --- | --- |
| 核心语言 | **Rust** | 内存可控、编译成单个静态库嵌入 SwiftUI、零分配热路径 |
| 遍历 | **`ignore` crate**（fd 同款） | Apache-2.0，并行，遵守 gitignore 语义 |
| 匹配 | **自移植 fzf `FuzzyMatchV2`** | 与 fzf 逐位一致，可用差分测试证明 |
| 并行 | **`rayon`** | 把 25 万候选切成多核 |
| 增量更新 | **macOS 原生 FSEvents**（CoreFoundation） | 不需要额外 crate |
| 持久化 | **`postcard` + `zstd`** | 快照 ~4MB，带版本号和校验和 |
| UI | **SwiftUI + AppKit** | 原生观感，`NSPanel` + vibrancy |
| 桥接 | **C ABI 静态库** | Swift 只做视图，核心在 Rust |

### 5.2 为什么是 C ABI 静态库而不是别的

- **不用 Tauri / Electron**：需要 Node 或 WebView，内存重，观感不原生
- **不用纯 Rust GUI**：macOS 上没有像样的 Rust GUI 生态，SwiftUI 才是原生观感的正解
- **C ABI 是最小公分母**：Rust 导出 `extern "C"`，Swift 通过 module map 调用。
  核心能编译成任何平台的静态库，UI 保持原生。

### 5.3 目录结构

```
core/src/
  fuzzy/      fzf 移植：评分、bonus 矩阵、重音折叠、排序      (~1,822 行)
  scan.rs     fd 式并行遍历
  index.rs    slab + interning 的紧凑索引
  corpus.rs   扁平可切片的可搜索结构
  search.rs   Rayon 并行搜索
  query.rs    fzf 查询语法解析
  persist.rs  快照格式
  watch.rs    FSEvents 监视器
  ffi.rs      C ABI

app/Sources/Seek/
  SeekApp.swift    应用入口、面板、热键
  ContentView.swift     面板布局
  ResultRow.swift       结果行（fzf 式高亮）
  AppModel.swift        状态机与后台任务生命周期
  SearchEngine.swift    C ABI 的 Swift 包装
  PreferencesView.swift  设置窗口
  Theme.swift           视觉规范
  SmokeTest.swift       端到端自检
```

---

## 6. 实现步骤（已完成 11 步）

| 步 | 内容 | 产出 | 验证 |
| --- | --- | --- | --- |
| 0 | 研究 Cardinal，确认 slab + interning + 版本化取消 | 选型表 | — |
| 1 | Rust 核心：fd 遍历 + fzf 匹配 + 排序 + 快照 | 3,800 行 | fzf 差分测试 |
| 2 | SwiftUI 面板 + C ABI 桥接 + Carbon 热键 | 1,400 行 | Swift 测试 |
| 3 | FSEvents 增量更新 | 560 行 | live_index / watcher 测试 |
| 4 | UI 第一轮重做（`Theme.swift`、vibrancy、行排版） | — | 快照渲染 |
| 5 | 修"输入没结果"（任务取消 + 批量 FFI） | — | 39 个 Swift 测试 |
| 6 | 修"面板根本不显示"（`hidesOnDeactivate` 时序 + `NSHostingController` 生命周期） | — | 端到端 delegate 测试 + 活体快照 |
| 7 | 把"fd 和 fzf 是核心"变成硬约束（Cursor 规则 + fd 差分测试） | — | 2 组差分测试全过 |
| 8 | 活体验证（`--query` / `--snapshot` 渲染真实 panel） | — | 活体 PNG 确认结果已绘制 |
| 9 | 修"面板一直是空的"真正的根因：**锁饥饿** | 引擎拆锁 + 快照发布 | 持锁搜索测试 + 像素断言 |
| 10 | 对齐 fzf 的 path scheme：扫描方向 + 真实匹配起点 | `scan_direction` + backtrace 定起点 | 35 个 pattern × 2 scheme 差分全过 |
| 11 | 性能优化（profile 驱动）：ASCII fold、memchr prefilter、只排前 limit、缓存 trim_length | 最慢查询 69 → 43 ms | 截断前缀一致性测试 |

详细的每一步、遇到的问题、根因分析见 **[DEVELOPMENT.md](DEVELOPMENT.md)**。

---

## 7. 当前状态

### 已验证

- ✅ fzf 排序逐位一致（差分测试，对真实 `fzf` 二进制；default 和 path 两种 scheme 都比）
- ✅ fd 遍历行为一致（差分测试，对真实 `fd` 二进制）
- ✅ 实时增量更新
- ✅ 性能：25.8 万条路径下 **1.6–43 ms**，全部在 F3 的 50 ms 预算内
  （同机 fzf 2.7–189 ms，快 1.7–11 倍）；**watcher 运行时**搜索 24–52 ms
- ✅ 内存：常驻 60.8 MB / 25.8 万条（索引 18.4 MB + corpus 42.4 MB，约 235 B/条）
- ✅ 匹配器真的零分配：scratch buffer 现在每 worker 一个，不是每条候选一个
- ✅ **面板显示结果** —— 结果行真的画在屏幕上（数结果区像素），
  且每一行都匹配所输入的查询，状态栏与列表一致
- ✅ 搜索不会被索引维护阻塞（持有维护锁时断言搜索仍有应答）
- ✅ 产物的部署目标与 Info.plist 一致（`minos 14.0`）
- ✅ 52 个 core 测试 + 8 个差分测试 + 46 个 Swift 测试

> 差分测试带 `#[ignore]`，因为它们需要真实的 `fd` / `fzf` 二进制：
> `cargo test --release -- --ignored`

### 待办（按优先级）

1. **UI 精修** —— 搜索已经进预算，现在它是最该做的一项
2. **继续优化**（可选，预算已达成）—— 窗口仍是 `Vec<char>`（ASCII 下每字符 4 字节），
   fzf 的 2 字符融合快路径（`fuzzyMatchV2Two`）也没移植
3. `content:` 操作符 —— 暂不实现（设置里已显示为禁用并标注 "not available yet"）

### 之前那个"留给你定的问题"——已经定了

原来的问题是：app 的 path scheme 要不要跟 fzf 的 `--scheme=path` 默认 tiebreak
对齐。答案是**要**，而且这根本不是产品选择 —— 按 F3 的说法它就是 bug。
`man fzf` 写得很清楚：`--scheme=path` 自己就会设 `--tiebreak=pathname,length`。
现在 scheme 自带链条（`Scheme::criteria`），两者不可能再走偏。

对齐的过程里挖出了两个真正的 bug，都不是"排序规则"层面的，而是**匹配位置**层面的：

1. **扫描方向** —— fzf 在 `core.go` 里按 tiebreak 链决定匹配器的方向，
   `pathname` 会把它设成**反向**（同分取最后一个匹配位置）。
   `libparse/src/main.rs` 里 `a` 可以命中 `libparse` 也可以命中 `main.rs`，
   分数完全一样；正向扫描会算成目录命中，把这一条排到所有文件名命中之后。
2. **匹配起点** —— 之前报的起点是 `f0`，也就是首字符**可能**落脚的第一个位置，
   不是匹配真正开始的地方。`libparse/src` 搜 `src`，`f0` 指向 `libparse` 里的 `s`。
   所有基于 span 的 tiebreak 都是从起点量的，所以这同样把文件名命中算成了目录命中。
   现在起点一律取自 backtrace（fzf 为此在链里需要 span 时会打开 `withPos`）。

差分测试也跟着改了。原来是逐位比 `fzf --filter` 的输出，这其实在比 fzf 的
**并行分块合并**，不是在比排序：输入量一大，`fzf --filter` 的尾部顺序自己
都不是全序（把它的输出再喂回 fzf，条目会挪位；单独问它这两条，它给的答案
和输出里的顺序相反）。现在的做法是：匹配**集合**必须完全一致；顺序上，
只要 app 的 key 和 fzf 打印的顺序有冲突，就把这两条单独交给 fzf 裁决 ——
那是 fzf 唯一精确的模式。
