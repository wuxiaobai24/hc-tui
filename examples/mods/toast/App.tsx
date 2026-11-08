// Toast：内置提示层的模块实现。
//
// 几何与内置 `render_toast` 同构：内容区右下角、宽 `min(码点数+4, 列数)`
// （最短 12 列）、3 行带边框盒；`level` 只改边框色（info→accent、
// warn→agent_working、error→agent_error）。多条按 `props.toasts` 的顺序
// 自下而上堆叠，最旧的一条在最下面；超出 `max` 的条目由宿主截断。
// 点击一条：带 `action` 时先 `run`（`target` 作 `terminal` 参数），再 `dismiss`。

import { host, mount, signal, Text, View } from "nsh";

type Item = {
  id: string;
  level: string;
  text: string;
  action?: unknown;
  target?: string;
  created_ms: number;
  timeout_ms: number;
};

const params = (host.params ?? {}) as any;
const cellW: number = params.cell?.[0] ?? 8;
const cellH: number = params.cell?.[1] ?? 16;

const toasts = signal<Item[]>([]);
const size = signal({ cells: 0, rows: 0 });
const tokens = signal<Record<string, string>>(params.theme?.tokens ?? {});
const error = signal("");

function token(name: string, fallback: string): string {
  return tokens()[name] ?? fallback;
}

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

/// 内置宽度公式按码点计数（`toast.text.chars().count()`），宽度再夹到内容区。
function boxWidth(text: string, cells: number): number {
  return Math.max(12, Math.min([...text].length + 4, cells));
}

function tint(level: string): string {
  if (level == "warn") return token("agent_working", "#f9e2af");
  if (level == "error") return token("agent_error", "#f38ba8");
  return token("accent", "#89b4fa");
}

async function activate(item: Item) {
  const action = item.action as any;
  const name = typeof action == "string" ? action : action?.name;
  if (name) {
    const args = {
      ...(typeof action == "object" && action?.args ? action.args : {}),
    };
    if (item.target != null && args.terminal == null) args.terminal = item.target;
    try {
      await host.call("run", { name, args });
    } catch (err) {
      error.set((err as Error).message);
    }
  }
  try {
    await host.call("dismiss", { id: item.id });
  } catch (err) {
    error.set((err as Error).message);
  }
}

type Box = { item: Item; level: string; left: number; width: number };

/// 自下而上堆叠：下标 0（最旧）在最下面，渲染顺序从上往下。
function boxRows(): Box[] {
  const list = toasts();
  const cells = size().cells;
  const rows = size().rows;
  if (cells < 12 || rows < 3) return [];
  const fit = Math.min(list.length, Math.floor(rows / 3));
  const out: Box[] = [];
  for (let index = fit - 1; index >= 0; index--) {
    const item = list[index];
    const width = boxWidth(item.text, cells);
    out.push({ item, level: item.level, left: cells - width, width });
  }
  return out;
}

function App() {
  host.on("props.changed", (payload: any) => {
    toasts.set(payload?.props?.toasts ?? []);
  });
  host.on("theme.changed", (theme: any) => {
    if (theme?.tokens) tokens.set(theme.tokens);
  });

  const rows = () => boxRows();
  const top = () => Math.max(0, size().rows - 3 * boxRows().length);

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
      <View style={{ height: top() * cellH }} />
      <Each of={rows()}>
        {(box) => (
          <View style={{ flexDirection: "column", height: 3 * cellH }}>
            <View style={{ flexDirection: "row", height: cellH }}>
              <View style={{ width: box().left * cellW }} />
              <Text style={{ color: tint(box().level) }}>
                {`┌${"─".repeat(box().width - 2)}┐`}
              </Text>
            </View>
            <View style={{ flexDirection: "row", height: cellH }}>
              <View style={{ width: box().left * cellW }} />
              <View
                onPress={() => activate(box().item)}
                role="button"
                style={{ flexDirection: "row", width: box().width * cellW }}
              >
                <Text style={{ color: tint(box().level) }}>{"│"}</Text>
                <Text style={{ color: token("toast_fg", "#cdd6f4") }}>
                  {pad(truncate(box().item.text, box().width - 3), box().width - 2)}
                </Text>
                <Text style={{ color: tint(box().level) }}>{"│"}</Text>
              </View>
            </View>
            <View style={{ flexDirection: "row", height: cellH }}>
              <View style={{ width: box().left * cellW }} />
              <Text style={{ color: tint(box().level) }}>
                {`└${"─".repeat(box().width - 2)}┘`}
              </Text>
            </View>
          </View>
        )}
      </Each>
      {error() ? (
        <Text style={{ color: token("agent_error", "#f38ba8") }}>{error()}</Text>
      ) : null}
    </View>
  );
}

mount(App);
