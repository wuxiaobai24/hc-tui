// Settings：内置设置浮层的模块实现。
//
// 面板几何与内置 `render_settings` 同构：面板宽 `min(60, surface-4)`、高
// `min(surface-2, 项数+9)`，在内容区居中；段标题在窄屏折成两行；项行
// `▸`/`✓`；底部按键提示。宿主把 surface 打开成内容区尺寸的 dialog，所以
// 模块拿到的是比内容区各少 2 列 / 2 行的矩形，面板在模块矩形内居中与在
// 内容区居中逐格等价。
//
// 行文本按终端列宽在 JS 里截断与补白，避免 nsh 在行尾自己加省略号。

import { host, keymap, mount, signal, Text, View } from "nsh";

type Item = { id: string; value: string; label: string; active: boolean };
type Section = { name: string; label: string };
type Settings = {
  section?: string;
  sections?: Section[];
  items?: Item[];
  current?: Record<string, unknown>;
};
type Snapshot = { settings?: Settings };

type Style = {
  text: string;
  color?: string;
  background?: string;
  bold?: boolean;
  onPress?: (event?: any) => void;
};

const params = (host.params ?? {}) as any;
const cellW: number = params.cell?.[0] ?? 8;
const cellH: number = params.cell?.[1] ?? 16;

const snapshot = signal<Snapshot>({});
const section = signal("");
const selected = signal(0);
const size = signal({ cells: 0, rows: 0 });
const error = signal("");
const tokens = signal<Record<string, string>>(params.theme?.tokens ?? {});
// 首份 state 到手前一行都不画：连接、主题与当前值未定时画出来的是半份面板。
const ready = signal(false);

function token(name: string, fallback: string): string {
  return tokens()[name] ?? fallback;
}

async function refresh() {
  try {
    const next = await host.call<Snapshot>("state", { domains: ["settings"] });
    snapshot.set(next);
    const settings = next.settings;
    if (settings?.section && !section()) {
      section.set(settings.section);
      selected.set(activeIndex(settings.section, settings));
    }
    ready.set(true);
    error.set("");
  } catch (err) {
    error.set((err as Error).message);
  }
}

function settings(): Settings {
  return snapshot().settings ?? {};
}

function sections(): Section[] {
  return settings().sections ?? [];
}

function items(name?: string): Item[] {
  const wanted = name ?? section();
  return (settings().items ?? []).filter((item) => item.id.startsWith(`${wanted}:`));
}

function activeIndex(name: string, from?: Settings): number {
  const list = from
    ? (from.items ?? []).filter((item) => item.id.startsWith(`${name}:`))
    : items(name);
  const index = list.findIndex((item) => item.active);
  return index < 0 ? 0 : index;
}

const ACTIONS: Record<string, string> = {
  theme: "app.theme",
  indicators: "app.indicators",
  toasts: "app.notifications",
  sound: "app.sound",
};

async function apply(item: Item) {
  const name = item.id.split(":")[0];
  const action = ACTIONS[name];
  if (!action) return;
  const args = name === "theme" ? { name: item.value } : { value: item.value };
  try {
    await host.call("run", { name: action, args });
    const index = items(name).findIndex((entry) => entry.id === item.id);
    if (index >= 0) selected.set(index);
    error.set("");
  } catch (err) {
    error.set((err as Error).message);
  }
}

function applySelected() {
  const item = items()[selected()];
  if (item) apply(item);
}

function move(delta: number) {
  const len = items().length;
  if (len == 0) return;
  selected.set(Math.max(0, Math.min(len - 1, selected() + delta)));
}

function pickTo(index: number) {
  const list = sections();
  if (index < 0 || index >= list.length) return;
  section.set(list[index].name);
  selected.set(activeIndex(list[index].name));
}

function pickSection(delta: number) {
  const list = sections();
  if (list.length == 0) return;
  const current = Math.max(
    0,
    list.findIndex((entry) => entry.name === section()),
  );
  pickTo((current + delta + list.length) % list.length);
}

function applyItem(index: number) {
  const item = items()[index];
  if (!item) return;
  selected.set(index);
  apply(item);
}

function close() {
  host.call("closeOverlay", { surface: params.surface }).catch(() => {});
}

// 按键在模块顶层绑定：组件重渲染不会撤掉这一层。
keymap.bind("j", () => move(1));
keymap.bind("down", () => move(1));
keymap.bind("k", () => move(-1));
keymap.bind("up", () => move(-1));
keymap.bind("tab", () => pickSection(1));
keymap.bind("shift+tab", () => pickSection(-1));
keymap.bind("right", () => pickSection(1));
keymap.bind("left", () => pickSection(-1));
keymap.bind("enter", () => applySelected());
keymap.bind("escape", () => close());
keymap.bind("q", () => close());

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

/// 与内置 `truncate` 同一算法：超宽时截到 `max-1` 列再补 `…`。
function truncate(text: string, max: number): string {
  if (displayWidth(text) <= max) return text;
  if (max == 0) return "";
  let out = "";
  let columns = 0;
  for (const ch of text) {
    const width = displayWidth(ch);
    if (columns + width > max - 1) break;
    out += ch;
    columns += width;
  }
  return out + "…";
}

function pad(text: string, width: number): string {
  const room = width - displayWidth(text);
  return room > 0 ? text + " ".repeat(room) : text;
}

type Spot = { section: number; start: number; width: number };

type TextStyle = {
  kind: "text";
  text: string;
  color?: string;
  background?: string;
  bold?: boolean;
  onPress?: () => void;
};

type TabsStyle = { kind: "tabs"; spots: Spot[] };

type Row = TextStyle | TabsStyle;

/// 段标题行；窄屏（内宽 < 48）按内置实现折成两行。`spots` 是每个段标题
/// 在行内的列范围。
function tabRows(inner: number): Spot[][] {
  const list = sections();
  const two = inner < 48;
  const rows: Spot[][] = [];
  let spots: Spot[] = [];
  let column = 1;
  for (let index = 0; index < list.length; index++) {
    if (two && index == 2) {
      rows.push(spots);
      spots = [];
      column = 1;
    }
    const width = Math.max(
      0,
      Math.min(displayWidth(` ${list[index].label} `), inner - column),
    );
    spots.push({ section: index, start: column, width });
    column += width + 1;
  }
  rows.push(spots);
  return rows;
}

/// 面板的每一行；文本行已带左右边框并补满面板宽度，段标题行留给可点的分段。
function panelRows(): Row[] {
  const cells = size().cells;
  const rows = size().rows;
  const width = Math.min(60, cells);
  const len = items().length;
  const height = Math.min(rows, len + 9);
  if (width < 24 || height < 8) return [];
  const inner = width - 2;
  const innerHeight = height - 2;
  const tabCount = inner < 48 ? 2 : 1;

  const border = token("border", "#6c7086");
  const fg = token("fg", "#cdd6f4");
  const dim = token("dim", "#7f849c");
  const selectionBg = token("selection_bg", "#585b70");
  const selectionFg = token("selection_fg", "#cdd6f4");

  const body: Row[] = [];
  body.push({ kind: "text", text: " Settings", color: fg, bold: true });
  body.push({ kind: "text", text: "" });
  for (const spots of tabRows(inner)) {
    body.push({ kind: "tabs", spots });
  }
  body.push({ kind: "text", text: "─".repeat(inner), color: border });

  const available = innerHeight - (5 + tabCount);
  const offset = Math.max(0, selected() - Math.max(available - 1, 0));
  const last = Math.min(len, offset + available);
  for (let index = offset; index < last; index++) {
    const item = items()[index];
    const current = index == selected();
    const marker = current ? "▸" : " ";
    const check = item.active ? "✓" : " ";
    const label = truncate(item.label, inner - 7);
    const room = inner - displayWidth(label) - 6;
    body.push({
      kind: "text",
      text: ` ${marker} ${label}${" ".repeat(Math.max(room, 0))} ${check} `,
      color: current ? selectionFg : fg,
      background: current ? selectionBg : undefined,
      onPress: () => applyItem(index),
    });
  }
  while (body.length < innerHeight - 1) {
    body.push({ kind: "text", text: "" });
  }
  const footer =
    inner >= 48
      ? " ↑↓ select   ←→ section   ↵ apply   esc close"
      : " ↑↓ select  tab section  ↵ apply  esc close";
  body.push({ kind: "text", text: footer, color: dim });

  const out: Row[] = [];
  out.push({ kind: "text", text: `┌${"─".repeat(width - 6)} [x] `, color: border });
  for (const line of body) {
    if (line.kind == "tabs") {
      out.push(line);
      continue;
    }
    out.push({
      kind: "text",
      text: `│${pad(truncate(line.text, inner), inner)}│`,
      color: line.color ?? fg,
      background: line.background,
      bold: line.bold,
      onPress: line.onPress,
    });
  }
  out.push({ kind: "text", text: `└${"─".repeat(width - 2)}┘`, color: border });
  return out;
}

function Line(props: { row: TextStyle }) {
  const row = props.row;
  return (
    <View
      onPress={() => row.onPress?.()}
      role={row.onPress ? "button" : undefined}
      style={{
        height: cellH,
        flexDirection: "row",
        backgroundColor: row.background,
      }}
    >
      <Text
        style={{
          color: row.color,
          backgroundColor: row.background,
          fontWeight: row.bold ? "bold" : "normal",
        }}
      >
        {row.text}
      </Text>
    </View>
  );
}

/// 段标题行：每个段标题是独立可点区域，前后留白与内置实现逐格一致。
function Tabs(props: { spots: Spot[] }) {
  const dim = token("dim", "#7f849c");
  const border = token("border", "#6c7086");
  const selectionBg = token("selection_bg", "#585b70");
  const selectionFg = token("selection_fg", "#cdd6f4");
  const list = sections();
  const current = list.findIndex((entry) => entry.name === section());
  return (
    <View
      style={{
        height: cellH,
        flexDirection: "row",
        backgroundColor: token("sidebar_bg", "#1e1e2e"),
      }}
    >
      <Text style={{ color: border }}>│</Text>
      <View style={{ width: cellW }} />
      <Each of={props.spots}>
        {(spot) => (
          <View style={{ flexDirection: "row" }}>
            <View
              onPress={() => pickTo(spot().section)}
              role="button"
              style={{
                width: spot().width * cellW,
                height: cellH,
                flexDirection: "row",
                backgroundColor:
                  spot().section == current ? selectionBg : undefined,
              }}
            >
              <Text
                style={{
                  color: spot().section == current ? selectionFg : dim,
                  fontWeight: spot().section == current ? "bold" : "normal",
                }}
              >
                {truncate(` ${list[spot().section].label} `, spot().width)}
              </Text>
            </View>
            <View style={{ width: cellW }} />
          </View>
        )}
      </Each>
      <View style={{ flex: 1 }} />
      <Text style={{ color: border }}>│</Text>
    </View>
  );
}

function App() {
  host.on("app.ready", refresh);
  host.on("state.changed", refresh);
  host.on("theme.changed", (theme: any) => {
    if (theme?.tokens) tokens.set(theme.tokens);
  });
  refresh();

  const rows = () => panelRows();
  const layout = () => {
    if (!ready()) return { width: 0, height: 0, left: 0, top: 0 };
    const cells = Math.max(0, size().cells);
    const count = Math.max(0, size().rows);
    const width = Math.min(60, cells);
    const len = items().length;
    const height = cells < 24 || count < 8 ? 0 : Math.min(count, len + 9);
    return {
      width,
      height,
      left: Math.floor((cells - width) / 2),
      top: Math.floor((count - height) / 2),
    };
  };

  return (
    <View
      onLayout={(event: any) => {
        const bounds = event?.bounds?.size ?? {};
        const next = {
          cells: Math.floor((bounds.width ?? 0) / cellW),
          rows: Math.floor((bounds.height ?? 0) / cellH),
        };
        if (next.cells !== size().cells || next.rows !== size().rows) size.set(next);
      }}
      style={{ flex: 1, flexDirection: "column" }}
    >
      <View style={{ height: layout().height > 0 ? layout().top * cellH : 0 }} />
      {layout().height > 0 ? (
        <View style={{ flexDirection: "row" }}>
          <View style={{ width: layout().left * cellW }} />
          <View
            style={{
              width: layout().width * cellW,
              height: layout().height * cellH,
              flexDirection: "column",
              backgroundColor: token("sidebar_bg", "#1e1e2e"),
            }}
          >
            <Each of={rows()}>
              {(row) =>
                row().kind == "tabs" ? (
                  <Tabs spots={row().spots} />
                ) : (
                  <Line row={row() as TextStyle} />
                )
              }
            </Each>
          </View>
        </View>
      ) : null}
      {error() ? (
        <Text style={{ color: token("agent_error", "#f38ba8") }}>{error()}</Text>
      ) : null}
    </View>
  );
}

mount(App);
