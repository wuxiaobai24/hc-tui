// Status：模式行 / 连接状态 / 快捷键提示。
//
// 状态行的文本（模式标签与提示）不在 `state` 里：模块按连接状态渲染宿主在无
// 模式、无提示时给出的那一行；模式行内容需要宿主 props（见票据「存疑」）。

import { host, mount, signal, Text, View } from "nsh";

type Snapshot = {
  connection?: string;
  focused?: { mod?: string | null };
};

type Segment = {
  text: string;
  token?: string;
  bg?: string;
  bold?: boolean;
};

const params: any = (host as any).params ?? {};
const tokens: Record<string, string> = params.theme?.tokens ?? {};
const cellW: number = params.cell?.[0] ?? 8;
const cellH: number = params.cell?.[1] ?? 16;
const zh = params.locale === "zh";

const DIM = "dim";
const WORKING = "agent_working";

const tr = (en: string, cn: string) => (zh ? cn : en);

const color = (token: string | undefined): string | undefined => {
  if (!token) return undefined;
  const hex = tokens[token];
  if (!hex) return undefined;
  // "000000" 是 Color::Reset 的序列化：留空，渲染器才会当默认色。
  if (hex === "000000") return undefined;
  return hex.startsWith("#") ? hex : `#${hex}`;
};

const snapshot = signal<Snapshot>({});
const size = signal({ cells: 0 });

async function refresh() {
  try {
    snapshot.set(
      await host.call<Snapshot>("state", { domains: ["focused"] }),
    );
  } catch (_error) {
    // 授予未到位时保持上一帧。
  }
}

host.on("app.ready", refresh);
host.on("state.changed", refresh);
host.on("focus.changed", refresh);
refresh();

/// 连接状态对应的文案：与内置 `Connection::label` 的取值一致。
function connectionLabel(connection: string): string {
  switch (connection) {
    case "connecting":
      return tr("connecting", "连接中");
    case "ready":
      return "";
    default:
      return tr("disconnected", "已断开");
  }
}

/// 右侧连接状态的文案；放不下时内置实现整个不画。
function rightLabel(cells: number): string {
  const text = connectionLabel(snapshot().connection ?? "offline");
  const right = text;
  const used = text.length + 3;
  if (cells <= used + right.length) return "";
  return right;
}

function segStyle(segment: Segment) {
  return {
    color: color(segment.token),
    backgroundColor: segment.bg,
    fontWeight: segment.bold ? ("bold" as const) : undefined,
    whiteSpace: "nowrap" as const,
  };
}

function layoutLine(): Segment[] {
  const connection = snapshot().connection ?? "offline";
  const text = connectionLabel(connection);
  return [
    { text: "  " },
    { text, token: DIM },
  ];
}

function App() {
  return (
    <View
      onLayout={(event: any) => {
        const bounds = event?.bounds?.size ?? {};
        const next = { cells: Math.floor((bounds.width ?? 0) / cellW) };
        if (next.cells !== size().cells) size.set(next);
      }}
      style={{ flex: 1, flexDirection: "row", justifyContent: "space-between" }}
    >
      <View style={{ flexDirection: "row" }}>
        <Each of={layoutLine()}>
          {(segment: any) => <Text style={segStyle(segment())}>{segment().text}</Text>}
        </Each>
      </View>
      <Text style={{ color: color(WORKING), whiteSpace: "nowrap" }}>
        {rightLabel(size().cells) ? `${rightLabel(size().cells)} ` : ""}
      </Text>
    </View>
  );
}

mount(App);
