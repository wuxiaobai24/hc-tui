// Help：内置键位浮层的模块实现。
//
// 宿主把内容区整个交给本模块（`popup` 形态的矩形正好等于内容区），模块自己
// 画满整块区域并清屏：外框、标题行、查询行、分隔线、`key`/`text` 两栏、
// 右侧滚动条列与页脚，与内置 `render_help` 逐格同构。
//
// 键位表、copy-mode 固定行、预设与全部文案都来自打开时的 props（首份在启动
// 参数里，之后走 `props.changed`）；模块不拼 i18n，也不读 `state`。查询、
// 选中行与滚动由模块自持；过滤规则与内置相同（大小写不敏感匹配按键串与
// 标题，命中时插一个 `matches` 分组）。
//
// 行文本在 JS 里按终端列宽裁剪与补白，避免 nsh 在行尾自己加省略号。

import { host, keymap, mount, signal, Text, View } from "nsh";

type Row = { key: string; title: string };
type Group = { label: string; rows: Row[] };
type HelpProps = {
  title?: string;
  preset?: string;
  filter?: string;
  footer?: string;
  matches?: string;
  groups?: Group[];
  copyMode?: Group;
};

type Seg = {
  text: string;
  width: number;
  color?: string;
  background?: string;
  bold?: boolean;
  onPress?: () => void;
};

const params = (host.params ?? {}) as any;
const cellW: number = params.cell?.[0] ?? 8;
const cellH: number = params.cell?.[1] ?? 16;

const initial = (params.props ?? {}) as HelpProps;
const pushed = signal<HelpProps>({});
const query = signal("");
const selected = signal(0);
const size = signal({ cells: 0, rows: 0 });
const tokens = signal<Record<string, string>>(params.theme?.tokens ?? {});

function props(): HelpProps {
  return { ...initial, ...pushed() };
}

function token(name: string, fallback: string): string {
  return tokens()[name] ?? fallback;
}

host.on("props.changed", (event: any) => {
  if (event && typeof event.props === "object" && event.props !== null) {
    pushed.set(event.props);
  }
});
host.on("theme.changed", (theme: any) => {
  if (theme?.tokens) tokens.set(theme.tokens);
});

type FlatRow = { section: boolean; key: string; text: string };

function pushGroup(out: FlatRow[], group: Group) {
  out.push({ section: true, key: "", text: group.label });
  for (const row of group.rows) {
    out.push({ section: false, key: row.key, text: row.title });
  }
}

/// 未过滤的全部行：五组键位之后接 copy-mode 固定行。
function allRows(): FlatRow[] {
  const out: FlatRow[] = [];
  for (const group of props().groups ?? []) pushGroup(out, group);
  if (props().copyMode) pushGroup(out, props().copyMode as Group);
  return out;
}

/// 与内置 `to_ascii_lowercase` 一致：只折 ASCII 大写。
function asciiLower(text: string): string {
  let out = "";
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    out += code >= 65 && code <= 90 ? String.fromCodePoint(code + 32) : ch;
  }
  return out;
}

/// 过滤后的行；查询为空时原样返回，无命中时给空表。
function view(): FlatRow[] {
  const rows = allRows();
  const needle = asciiLower(query());
  if (!needle) return rows;
  const matched = rows.filter(
    (row) =>
      !row.section &&
      (asciiLower(row.key).includes(needle) ||
        asciiLower(row.text).includes(needle)),
  );
  if (matched.length == 0) return [];
  return [
    { section: true, key: "", text: props().matches ?? "matches" },
    ...matched,
  ];
}

function close() {
  host.call("closeOverlay", { surface: params.surface }).catch(() => {});
}

function move(delta: number) {
  const len = view().length;
  if (len == 0) return;
  selected.set(Math.max(0, Math.min(len - 1, selected() + delta)));
}

function pushText(text: string) {
  query.set(query() + text);
  selected.set(0);
}

function backspace() {
  query.set(Array.from(query()).slice(0, -1).join(""));
  selected.set(0);
}

function typeQuery(ch: string) {
  if (ch == "q" && !query()) {
    close();
    return;
  }
  pushText(ch);
}

// 按键在模块顶层绑定：组件重渲染不会撤掉这一层。
keymap.bind({
  escape: close,
  enter: close,
  backspace: backspace,
  j: () => move(1),
  down: () => move(1),
  k: () => move(-1),
  up: () => move(-1),
  pageup: () => move(-10),
  pagedown: () => move(10),
});
keymap.bind({ q: () => typeQuery("q") }, { repeat: true });

// 查询输入：内置按字符追加，`j`/`k` 永远是移动、`/` 被吞掉；这里用 keymap
// 覆盖同样的可打印键（US 布局的无修饰字符与 Shift 字符）。
const LETTERS = "abcdefghijklmnopqrstuvwxyz";
const DIGITS = "0123456789";
const SHIFT_DIGITS = ")!@#$%^&*(";
const PUNCTUATION = [
  ["`", "~"],
  ["-", "_"],
  ["=", "+"],
  ["[", "{"],
  ["]", "}"],
  ["\\", "|"],
  [";", ":"],
  ["'", "\""],
  [",", "<"],
  [".", ">"],
  ["/", "?"],
];
const typed: Record<string, () => void> = {};
for (const ch of LETTERS) {
  if (ch == "j" || ch == "k" || ch == "q") continue;
  typed[ch] = () => pushText(ch);
  typed[`shift+${ch}`] = () => pushText(ch.toUpperCase());
}
for (const digit of DIGITS) {
  typed[digit] = () => pushText(digit);
  typed[`shift+${digit}`] = () => pushText(SHIFT_DIGITS[DIGITS.indexOf(digit)]);
}
for (const [base, shifted] of PUNCTUATION) {
  if (base == "/") {
    typed[base] = () => {};
  } else {
    typed[base] = () => pushText(base);
  }
  typed[`shift+${base}`] = () => pushText(shifted);
}
typed["shift+j"] = () => pushText("J");
typed["shift+k"] = () => pushText("K");
typed["shift+q"] = () => pushText("Q");
typed["space"] = () => pushText(" ");
typed["shift+space"] = () => pushText(" ");
keymap.bind(typed, { repeat: true });

function displayWidth(text: string): number {
  let width = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    const wide =
      (code >= 0x1100 && code <= 0x115f) ||
      (code >= 0x2e80 && code <= 0xa4cf) ||
      (code >= 0xac00 && code <= 0xd7a3) ||
      (code >= 0xf900 && code <= 0xfaff) ||
      (code >= 0xfe30 && code <= 0xfe6f) ||
      (code >= 0xff00 && code <= 0xff60) ||
      (code >= 0xffe0 && code <= 0xffe6) ||
      (code >= 0x20000 && code <= 0x3fffd);
    width += wide ? 2 : 1;
  }
  return width;
}

/// 硬裁到 `max` 列：内置的 `Paragraph` 在区域右缘直接截断，不补省略号。
function clip(text: string, max: number): string {
  let out = "";
  let columns = 0;
  for (const ch of text) {
    const width = displayWidth(ch);
    if (columns + width > max) break;
    out += ch;
    columns += width;
  }
  return out;
}

/// 与内置 `{:28}` 一致：按键串按字符数补到 28。
function padKey(text: string): string {
  return text.length >= 28 ? text : text + " ".repeat(28 - text.length);
}

/// 一行的内容格，两侧补上左右边框；内容不够宽时补空格。
function frameRow(content: Seg[], inner: number, accent: string): Seg[] {
  const out: Seg[] = [{ text: "│", width: 1, color: accent }];
  let room = inner;
  for (const seg of content) {
    if (room <= 0) break;
    if (seg.width <= room) {
      out.push(seg);
      room -= seg.width;
      continue;
    }
    const text = clip(seg.text, room);
    const width = displayWidth(text);
    if (width > 0) out.push({ ...seg, text, width });
    room -= width;
  }
  if (room > 0) out.push({ text: " ".repeat(room), width: room });
  out.push({ text: "│", width: 1, color: accent });
  return out;
}

/// 面板的每一行；`cells`/`rows` 是内容区的终端格数。
function rowsFor(cells: number, rows: number): Seg[][] {
  if (cells < 20 || rows < 6) return [];
  const values = props();
  const accent = token("accent", "#89b4fa");
  const border = token("border", "#45475a");
  const dim = token("dim", "#7f849c");
  const fg = token("fg", "#cdd6f4");
  const selectionBg = token("selection_bg", "#585b70");
  const selectionFg = token("selection_fg", "#cdd6f4");
  const base = token("sidebar_bg", "#1e1e2e");
  const inner = cells - 2;
  const bodyRows = rows - 6;
  const list = view();
  const scroll =
    selected() >= bodyRows ? selected() + 1 - bodyRows : 0;

  const out: Seg[][] = [];
  // 顶边框：右上五列是内置 `pointer_close` 覆盖的关闭按钮。
  out.push([
    { text: "┌", width: 1, color: accent },
    { text: "─".repeat(cells - 6), width: cells - 6, color: accent },
    {
      text: " [x] ",
      width: 5,
      color: selectionFg,
      background: selectionBg,
      onPress: close,
    },
  ]);
  // 标题行：左侧标题（暗色加粗）与右侧预设。
  const title = values.title ?? " keybinds";
  const preset = values.preset ?? "";
  const gap = Math.max(
    0,
    inner - displayWidth(title) - displayWidth(preset),
  );
  out.push(
    frameRow(
      [
        { text: title, width: displayWidth(title), color: dim, bold: true },
        { text: " ".repeat(gap), width: gap },
        { text: preset, width: displayWidth(preset), color: dim },
      ],
      inner,
      accent,
    ),
  );
  // 查询行。
  const needle = query();
  out.push(
    frameRow(
      needle
        ? [
            { text: "  / ", width: 4, color: accent },
            { text: needle, width: displayWidth(needle), color: fg },
          ]
        : [
            { text: "  /", width: 3, color: accent },
            {
              text: values.filter ?? " press / to filter by command or shortcut",
              width: displayWidth(
                values.filter ?? " press / to filter by command or shortcut",
              ),
              color: dim,
            },
          ],
      inner,
      accent,
    ),
  );
  // 分隔线：整行一个文本，避免 nsh 把孤立横线当成 frame 的 divider 而与
  // 两侧边框接成 `├`/`┤`（内置的 Paragraph 不做这种合并）。
  out.push([
    {
      text: `│${"─".repeat(inner)}│`,
      width: cells,
      color: border,
    },
  ]);
  // 正文：`key`/`text` 两栏，右侧滚动条列按内置的推导给位置。
  const shown: Seg[][] = [];
  for (
    let index = scroll;
    index < Math.min(list.length, scroll + bodyRows);
    index++
  ) {
    const row = list[index];
    const active = index == selected();
    const text = row.section
      ? ` ${row.text}`
      : ` ${padKey(row.key)} ${row.text}`;
    const color = active ? selectionFg : row.section ? accent : fg;
    const background = active ? selectionBg : undefined;
    const bold = !active && row.section;
    if (displayWidth(text) >= inner) {
      const cut = clip(text, inner);
      shown.push([
        { text: cut, width: displayWidth(cut), color, background, bold },
      ]);
      continue;
    }
    const pad = inner - displayWidth(text) - 1;
    const track = bodyRows > 0 ? Math.floor(((index - scroll) * 5) / bodyRows) : 0;
    const content: Seg[] = [
      { text, width: displayWidth(text), color, background, bold },
    ];
    if (pad > 0) {
      content.push({
        text: " ".repeat(pad),
        width: pad,
        color: dim,
        background: base,
      });
    }
    content.push({
      text: track % 5 == 0 ? "▐" : " ",
      width: 1,
      color: accent,
    });
    shown.push(content);
  }
  for (const content of shown) out.push(frameRow(content, inner, accent));
  // 页脚：紧跟正文，剩下的行留白到底边框（与内置的 Paragraph 行序一致）。
  const footer = values.footer ?? "";
  out.push(
    frameRow(
      [{ text: ` ${footer}`, width: displayWidth(` ${footer}`), color: dim }],
      inner,
      accent,
    ),
  );
  for (let fill = shown.length; fill < bodyRows; fill++) {
    out.push(frameRow([], inner, accent));
  }
  // 底边框。
  out.push([
    { text: "└", width: 1, color: accent },
    { text: "─".repeat(cells - 2), width: cells - 2, color: accent },
    { text: "┘", width: 1, color: accent },
  ]);
  return out;
}

function Line(props: { segs: Seg[] }) {
  const base = token("sidebar_bg", "#1e1e2e");
  return (
    <View
      style={{
        height: cellH,
        flexDirection: "row",
        backgroundColor: base,
      }}
    >
      <Each of={props.segs}>
        {(seg) => (
          <View
            onPress={seg().onPress ? () => seg().onPress?.() : undefined}
            role={seg().onPress ? "button" : undefined}
            style={{
              width: seg().width * cellW,
              height: cellH,
              flexDirection: "row",
              backgroundColor: seg().background,
            }}
          >
            <Text
              style={{
                color: seg().color,
                backgroundColor: seg().background,
                fontWeight: seg().bold ? "bold" : "normal",
              }}
            >
              {seg().text}
            </Text>
          </View>
        )}
      </Each>
    </View>
  );
}

function App() {
  return (
    <View
      onLayout={(event: any) => {
        const bounds = event?.bounds?.size ?? {};
        const next = {
          cells: Math.floor((bounds.width ?? 0) / cellW),
          rows: Math.floor((bounds.height ?? 0) / cellH),
        };
        if (next.cells !== size().cells || next.rows !== size().rows) {
          size.set(next);
        }
      }}
      style={{
        flex: 1,
        flexDirection: "column",
        backgroundColor: token("sidebar_bg", "#1e1e2e"),
      }}
    >
      <Each of={rowsFor(size().cells, size().rows)}>
        {(line) => <Line segs={line()} />}
      </Each>
    </View>
  );
}

mount(App);
