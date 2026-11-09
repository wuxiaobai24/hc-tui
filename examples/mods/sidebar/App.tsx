// Sidebar：Spaces + Agents + 折叠组 + 状态点，按 `state` 快照渲染。
//
// 绘制约束来自 nsh 的 TUI 渲染器：颜色按 token 取，行内文本按区域宽度在 JS 里
// 截断；每个非默认底色用带 `backgroundColor` 的 Text 覆盖。布局全部靠弹性盒，
// 尺寸由 `onLayout` 给的点数除以 `host.params.cell` 得到格子数。

import { host, mount, signal, Text, View } from "nsh";

type Space = {
  id: string;
  name?: string;
  machine?: string | null;
  folder?: string | null;
  tabs?: number;
};

type Agent = {
  address: string;
  name?: string | null;
  title?: string | null;
  turn?: number;
  pinned?: boolean;
};

type Snapshot = {
  machines?: string[];
  spaces?: Space[];
  agents?: Agent[];
  focused?: { space?: string | null };
};

type Segment = {
  text: string;
  token?: string;
  bg?: string;
  bold?: boolean;
};

type Row = {
  left: Segment[];
  right: Segment[];
  // 横线行自带边框列：nsh 会把相邻的 `─` 与 `│` 接成 `┤`，同一段文本里就不会接。
  noBar?: boolean;
};

const params: any = (host as any).params ?? {};
const tokens: Record<string, string> = params.theme?.tokens ?? {};
const cellW: number = params.cell?.[0] ?? 8;
const cellH: number = params.cell?.[1] ?? 16;
const zh = params.locale === "zh";

const tr = (en: string, cn: string) => (zh ? cn : en);

const DIM = "dim";
const FG = "fg";
const BORDER = "border";
const ACCENT = "accent";
const BASE = "bg";
const SELECTED = "sidebar_selected_bg";
const ICON_FG = "space_icon_fg";
const DONE = "agent_done";
const BADGE_GLYPH: Record<string, [string, string]> = {
  blocked: ["!", "◆"],
  error: ["x", "✖"],
  done: ["✓", "✔"],
  working: ["●", "◐"],
  idle: ["○", "○"],
  unknown: ["?", "◌"],
  none: ["·", "·"],
};
const BADGE_TOKEN: Record<string, string> = {
  blocked: "agent_blocked",
  error: "agent_error",
  done: "agent_done",
  working: "agent_working",
  idle: "agent_idle",
  unknown: "agent_unknown",
  none: "agent_unknown",
};
const SELECTION_BG = "selection_bg";
const SELECTION_FG = "selection_fg";
const SPACE_COLORS = [
  "space_teal",
  "space_green",
  "space_blue",
  "space_pink",
  "space_orange",
  "space_purple",
];

const color = (token: string | undefined): string | undefined => {
  if (!token) return undefined;
  const hex = tokens[token];
  if (!hex) return undefined;
  // "000000" 是 Color::Reset 的序列化：给宿主主色时留空，渲染器才会当默认色。
  if (hex === "000000") return undefined;
  return hex.startsWith("#") ? hex : `#${hex}`;
};

// 底色与基准底色相同的区域不重画：没被模块碰过的格子就是区域底色。
const ground = (token: string | undefined): string | undefined =>
  token && token !== BASE ? color(token) : undefined;

const wide = (code: number): boolean =>
  (code >= 0x1100 && code <= 0x115f) ||
  (code >= 0x2e80 && code <= 0xa4cf) ||
  (code >= 0xac00 && code <= 0xd7a3) ||
  (code >= 0xf900 && code <= 0xfaff) ||
  (code >= 0xfe30 && code <= 0xfe6f) ||
  (code >= 0xff00 && code <= 0xff60) ||
  (code >= 0xffe0 && code <= 0xffe6) ||
  (code >= 0x1f300 && code <= 0x1f64f) ||
  (code >= 0x1f900 && code <= 0x1f9ff);

const width = (text: string): number => {
  let total = 0;
  for (const char of text) total += wide(char.codePointAt(0) ?? 0) ? 2 : 1;
  return total;
};

const truncate = (text: string, max: number): string => {
  if (width(text) <= max) return text;
  let out = "";
  let used = 0;
  for (const char of text) {
    const step = wide(char.codePointAt(0) ?? 0) ? 2 : 1;
    if (used + step > max) break;
    out += char;
    used += step;
  }
  return out;
};

const pad = (count: number): string => (count > 0 ? " ".repeat(count) : "");

// 与侧栏内置实现的 hash 同构：identity 的 UTF-8 字节按 wrapping_mul(31) 累加。
const utf8 = (text: string): number[] => {
  const bytes: number[] = [];
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    if (code < 0x80) bytes.push(code);
    else if (code < 0x800) bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
    else if (code < 0x10000)
      bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
    else
      bytes.push(
        0xf0 | (code >> 18),
        0x80 | ((code >> 12) & 0x3f),
        0x80 | ((code >> 6) & 0x3f),
        0x80 | (code & 0x3f),
      );
  }
  return bytes;
};

const iconIndex = (identity: string): number => {
  let hash = 0n;
  for (const byte of utf8(identity)) {
    hash = (hash * 31n + BigInt(byte)) & 0xffffffffffffffffn;
  }
  return Number(hash % BigInt(SPACE_COLORS.length));
};

const snapshot = signal<Snapshot>({});
// 折叠状态不在 `state` 里：宿主把机器组与 Agents 分组折叠后作为 props 推过来。
const size = signal({ cells: 0, rows: 0 });
// 启动参数里的 props 只是首份；宿主用 `props.changed` 推最新的 Agents 分组。
const pushed = signal<Record<string, any>>({});
const ready = signal(false);

const props = (): any => ({ ...(params.props ?? {}), ...pushed() });

host.on("props.changed", (event: any) => {
  if (event && typeof event.props === "object" && event.props !== null) {
    pushed.set(event.props);
  }
});


const failure = signal("");
// 首份 state 到手前一行都不画：这段时间区域由宿主的内置实现顶着，不会上屏
// 半份内容（只有标题、没有 Space 与 Agent 行）。
async function refresh() {
  try {
    const reply = await host.call<Snapshot>("state", {
      domains: ["machines", "spaces", "agents", "focused"],
    });
    snapshot.set(reply);
    ready.set(true);
    failure.set("");
  } catch (error) {
    failure.set(String((error as Error)?.message ?? error));
  }
}

host.on("app.ready", refresh);
host.on("state.changed", refresh);
host.on("focus.changed", refresh);
refresh();

function spaceRow(space: Space, active: boolean, inner: number): Row {
  const label = space.name || space.id;
  const initial = truncate((label.charAt(0) || " ").toUpperCase(), 1);
  const identity = space.name || label;
  const chip = SPACE_COLORS[iconIndex(identity)];
  const name = truncate(label, Math.max(inner - 5, 0));
  const nameWidth = width(name);
  // 分支与待处理数不在 state 里：详情按空串处理，尾部补齐到区域宽度。
  const detail = "";
  const detailWidth = Math.max(inner - nameWidth - 6, 0);
  const tail = Math.max(inner - nameWidth - width(detail) - 5, 0);
  const style = active ? ground(SELECTED) : undefined;
  return {
    left: [
      { text: " ", bg: style, token: FG },
      { text: ` ${initial} `, bg: color(chip), token: ICON_FG, bold: true },
      { text: ` ${name}`, bg: style, token: FG, bold: true },
      { text: pad(tail) + truncate(detail, detailWidth), bg: style, token: DIM },
    ],
    right: [],
  };
}

function machineRow(machine: string, connected: boolean, inner: number, folded: boolean): Row {
  const name = truncate(
    machine === "local" ? tr("This computer", "本机") : machine,
    Math.max(inner - 7, 0),
  );
  const indent = Math.max(inner - width(name) - 6, 0);
  const chevron = folded ? "▸" : "▾";
  return {
    left: [
      { text: ` ${chevron} ${name}${pad(indent)}`, token: DIM },
      { text: connected ? "●" : "○", token: connected ? DONE : DIM },
      { text: "  ", token: DIM },
    ],
    right: [],
  };
}

function navTitle(left: string, right: string): Row {
  return {
    left: [{ text: left, token: DIM, bold: true }],
    right: right ? [{ text: right, token: DIM }] : [],
  };
}

const blank = (): Row => ({ left: [], right: [] });

/// Agents 段：宿主给的分组行 + 折叠状态，按内置实现的组头、徽标与选中底色渲染。
function agentRows(propsValue: any, inner: number): Row[] {
  const rows: Array<Record<string, any>> = propsValue.agents?.rows ?? [];
  const foldedGroups: string[] = propsValue.agents?.folded ?? [];
  const symbols = propsValue.indicators === "symbols";
  const rowsOut: Row[] = [];
  let previous = "";
  for (const row of rows) {
    const groupId = row.group_id ?? "";
    if (groupId !== previous) {
      const group = truncate(row.group ?? "", Math.max(inner - 4, 0));
      const chevron = foldedGroups.includes(groupId) ? "▸" : "▾";
      const text = ` ${chevron} ${group}`;
      rowsOut.push({
        left: [
          { text: text + pad(Math.max(inner - width(text), 0)), token: DIM },
        ],
        right: [],
      });
      previous = groupId;
    }
    if (foldedGroups.includes(groupId)) continue;
    const badge = String(row.badge ?? "none");
    const glyph = (BADGE_GLYPH[badge] ?? BADGE_GLYPH.none)[symbols ? 1 : 0];
    const label = truncate(row.label ?? "", Math.max(inner - 4, 0));
    const text = ` ${glyph} ${label}`;
    const focused = row.focused === true;
    rowsOut.push({
      left: [
        {
          text: text + pad(Math.max(inner - width(text), 0)),
          token: focused ? SELECTION_FG : BADGE_TOKEN[badge] ?? FG,
          bg: focused ? ground(SELECTION_BG) : undefined,
        },
      ],
      right: [],
    });
  }
  return rowsOut;
}

function rowsFor(snapshotValue: Snapshot, propsValue: any, inner: number, height: number): Row[] {
  const spaces = snapshotValue.spaces ?? [];
  const focused = snapshotValue.focused?.space ?? undefined;
  const machines: string[] = [];
  for (const space of spaces) machines.push(space.machine ?? "local");
  for (const machine of snapshotValue.machines ?? []) machines.push(machine);
  const unique: string[] = [];
  for (const machine of machines) {
    if (!unique.includes(machine)) unique.push(machine);
  }
  unique.sort((a, b) => {
    const left = a === "local" ? 0 : 1;
    const right = b === "local" ? 0 : 1;
    if (left !== right) return left - right;
    return a < b ? -1 : a > b ? 1 : 0;
  });

  const foldedMachines: string[] = propsValue.spaces?.folded ?? [];
  const rows: Row[] = [navTitle(tr(" Spaces", " 工作区"), "+ "), blank()];
  const spaceRows: Row[] = [];
  for (const machine of unique) {
    if (spaceRows.length > 0) spaceRows.push(blank());
    const group = `machine:${machine}`;
    const isFolded = foldedMachines.includes(group);
    const connected = machine === "local";
    const here = spaces.filter((space) => (space.machine ?? "local") === machine);
    spaceRows.push(machineRow(machine, connected, inner, isFolded));
    if (isFolded) continue;
    if (here.length === 0) {
      spaceRows.push({
        left: [
          {
            text: connected
              ? tr(" + New space there", " + 在那里新建工作区")
              : tr(" ↻ Reconnect", " ↻ 重新连接"),
            token: DIM,
          },
        ],
        right: [],
      });
    }
    for (const space of here) {
      spaceRows.push(spaceRow(space, space.id === focused, inner));
    }
  }
  const room = Math.min(spaceRows.length, Math.max(height - 10, 3));
  rows.push(...spaceRows.slice(0, room));
  rows.push(blank());

  const menu = `●${tr(" menu ", " 菜单 ")}`;
  rows.push({
    left: [{ text: tr(" new", " 新建"), token: ACCENT, bold: true }],
    right: [
      { text: "●", token: ACCENT, bold: true },
      { text: tr(" menu ", " 菜单 "), token: "tab_bar_fg" },
    ],
  });
  rows.push({
    left: [{ text: `${"─".repeat(inner)}│`, token: BORDER }],
    right: [],
    noBar: true,
  });
  const sort =
    propsValue.sort === "priority"
      ? tr("priority", "优先级")
      : tr("grouped", "分组");
  rows.push(navTitle(tr(" Agents", " Agent"), sort));

  const agents = agentRows(propsValue, inner);
  const agentRoom = Math.max(height - rows.length, 0);
  if (agents.length === 0) {
    rows.push({ left: [{ text: tr("  (no agents)", "  （无 Agent）"), token: DIM }], right: [] });
  } else {
    rows.push(...agents.slice(0, agentRoom));
  }
  while (rows.length < height) rows.push(blank());
  return rows;
}

function segStyle(segment: Segment) {
  return {
    color: color(segment.token),
    fontWeight: segment.bold ? ("bold" as const) : undefined,
    whiteSpace: "nowrap" as const,
  };
}

// 底色画在包住字形串的 View 上：文字自带底色会让渲染器把字形整格反显。
type Group = { bg?: string; segments: Segment[] };

function groups(segments: Segment[]): Group[] {
  const out: Group[] = [];
  for (const segment of segments) {
    const last = out[out.length - 1];
    if (last && last.bg === segment.bg) last.segments.push(segment);
    else out.push({ bg: segment.bg, segments: [segment] });
  }
  return out;
}

function GroupView(props: { group: Group }) {
  const group = props.group;
  return (
    <View
      style={{
        flexDirection: "row",
        backgroundColor: group.bg,
        overflow: "hidden",
      }}
    >
      <Each of={group.segments}>
        {(segment: any) => <Text style={segStyle(segment())}>{segment().text}</Text>}
      </Each>
    </View>
  );
}

function Line(props: { row: Row }) {
  const row = props.row;
  return (
    <View
      style={{
        flexDirection: "row",
        width: "100%",
        height: cellH,
        justifyContent: "space-between",
        overflow: "hidden",
      }}
    >
      <View style={{ flexDirection: "row", overflow: "hidden" }}>
        <Each of={groups(row.left)}>
          {(group: any) => <GroupView group={group()} />}
        </Each>
      </View>
      <View style={{ flexDirection: "row" }}>
        <Each of={groups(row.right)}>
          {(group: any) => <GroupView group={group()} />}
        </Each>
        {row.noBar ? null : (
          <Text style={{ color: color(BORDER), whiteSpace: "nowrap" }}>│</Text>
        )}
      </View>
    </View>
  );
}

function layoutRows(): Row[] {
  const current = size();
  if (!ready()) return [];
  if (current.cells <= 0 || current.rows <= 0) return [];
  return rowsFor(snapshot(), props(), Math.max(current.cells - 1, 0), current.rows);
}

function App() {
  let cells = 0;
  let rowCount = 0;
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
      <Each of={layoutRows()}>{(row: any) => <Line row={row()} />}</Each>
    </View>
  );
}

mount(App);
