// Tabs：Tab 栏，含上/下两种位置与右端的菜单块。
//
// 数据来自 `state` 的 `tabs` / `focused`；Tab 标签、选中底色与粗体按内置实现的
// 规则在 JS 里拼出，右侧状态文本与缩放标记依赖宿主 props（见票据「存疑」）。

import { host, mount, signal, Text, View } from "nsh";

type Tab = {
  id: string;
  space: string;
  name?: string | null;
  settled?: boolean;
};

type Snapshot = {
  tabs?: Tab[];
  focused?: { space?: string | null; tab?: string | null };
};

type Segment = {
  text: string;
  token?: string;
  bg?: string;
  bold?: boolean;
};

type Group = { bg?: string; bold?: boolean; segments: Segment[] };

const params: any = (host as any).params ?? {};
const tokens: Record<string, string> = params.theme?.tokens ?? {};
const cellW: number = params.cell?.[0] ?? 8;
const cellH: number = params.cell?.[1] ?? 16;
const zh = params.locale === "zh";

const DIM = "dim";
const TAB_BAR_FG = "tab_bar_fg";
const ACTIVE_BG = "tab_active_bg";
const ACTIVE_FG = "tab_active_fg";
const ACCENT = "accent";

const tr = (en: string, cn: string) => (zh ? cn : en);

const color = (token: string | undefined): string | undefined => {
  if (!token) return undefined;
  const hex = tokens[token];
  if (!hex) return undefined;
  // "000000" 是 Color::Reset 的序列化：留空，渲染器才会当默认色。
  if (hex === "000000") return undefined;
  return hex.startsWith("#") ? hex : `#${hex}`;
};

const ground = (token: string | undefined): string | undefined => (token ? color(token) : undefined);

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

const snapshot = signal<Snapshot>({});
const size = signal({ cells: 0, rows: 0 });

async function refresh() {
  try {
    snapshot.set(
      await host.call<Snapshot>("state", { domains: ["tabs", "focused"] }),
    );
  } catch (_error) {
    // 授予未到位时保持上一帧。
  }
}

host.on("app.ready", refresh);
host.on("state.changed", refresh);
host.on("focus.changed", refresh);
refresh();

/// Tab 栏的标签段：`  ` + 每个 Tab 的 ` 标签 缩放 标记 ` + 可选的 ` + `。
function tabGroups(snapshotValue: Snapshot, areaWidth: number): Group[] {
  const groups: Group[] = [{ segments: [{ text: "  " }] }];
  const space = snapshotValue.focused?.space ?? undefined;
  const active = snapshotValue.focused?.tab ?? undefined;
  const tabs = (snapshotValue.tabs ?? []).filter((tab) => tab.space === space);
  let x = 2;
  for (let index = 0; index < tabs.length; index += 1) {
    const tab = tabs[index];
    if (tab.settled) continue;
    const selected = tab.id === active;
    // 缩放标记与徽标不在 state 里：按未缩放、无徽标渲染，宽度照旧保留。
    const label =
      tab.name && tab.name.length > 0 ? truncate(tab.name, 20) : `${index + 1}`;
    const zoom = " ";
    const flag = " ";
    const text = ` ${label} ${zoom} ${flag} `;
    x += text.length;
    groups.push({
      bg: selected ? ground(ACTIVE_BG) : undefined,
      bold: selected,
      segments: [{ text, token: selected ? ACTIVE_FG : TAB_BAR_FG }],
    });
  }
  if (x < areaWidth) {
    groups.push({ segments: [{ text: " + ", token: DIM }] });
  }
  return groups;
}

function segStyle(segment: Segment, bold: boolean) {
  return {
    color: color(segment.token),
    fontWeight: bold || segment.bold ? ("bold" as const) : undefined,
    whiteSpace: "nowrap" as const,
  };
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
        {(segment: any) => (
          <Text style={segStyle(segment(), group.bold === true)}>{segment().text}</Text>
        )}
      </Each>
    </View>
  );
}

function layoutBar(): { bar: Group[]; menuCells: number } {
  const current = size();
  const cells = current.cells;
  if (cells <= 0) return { bar: [], menuCells: 0 };
  const menuCells = Math.min(8, cells);
  return { bar: tabGroups(snapshot(), cells - menuCells), menuCells };
}

function App() {
  const menuWorld = () => Math.min(8, size().cells);
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
      style={{ flex: 1, flexDirection: "row" }}
    >
      <View
        style={{
          flexDirection: "row",
          width: Math.max(size().cells - menuWorld(), 0) * cellW,
          overflow: "hidden",
        }}
      >
        <Each of={layoutBar().bar}>{(group: any) => <GroupView group={group()} />}</Each>
      </View>
      <View
        style={{
          flexDirection: "row",
          width: menuWorld() * cellW,
          backgroundColor: ground(ACCENT),
        }}
      >
        <Text style={{ color: color(ACTIVE_FG), whiteSpace: "nowrap" }}>
          {tr(" menu ", " 菜单 ")}
        </Text>
      </View>
    </View>
  );
}

mount(App);
