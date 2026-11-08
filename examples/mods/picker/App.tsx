import { effect, host, keymap, mount, Scroll, signal, Text, View } from "nsh";

// 宿主把候选行归一后放进 props：模块只做过滤、选中、滚动与 run({name,args})。
const params = host.params ?? {};
const props = params.props ?? {};
const form = props.form ?? "chooser";
const rows = props.rows ?? [];
const cols = props.cols ?? 20;
const height = props.height ?? 1;
const rowHeight = props.rowHeight ?? 1;
const visible = props.visible ?? 1;
const machines = props.machines === true;
const tokens = params.theme?.tokens ?? {};

const CELL_W = 8;
const CELL_H = 16;
const INNER_W = Math.max(0, cols - 2);
const INNER_H = Math.max(0, height - 2);
const CLOSE = " [x] ";

// 对拍反例开关：正常构建保持 0；测试注入 +1 时渲染必须与 builtin 不同。
const DEBUG_SKIP_ROW = 0;
const DEBUG_SHIFT_COL = 0;

const query = signal("");
const selected = signal(typeof props.selected === "number" ? props.selected : 0);

let scroller: any = null;
let autoScroll = -1;
let lastOffset = -1;
let wheelTimer: any = 0;

const color = (name: string): string | undefined => tokens[name];

function codepointWidth(code: number): number {
  if (code === 0) return 0;
  if (
    (code >= 0x1100 && code <= 0x115f) ||
    (code >= 0x2e80 && code <= 0xa4cf) ||
    (code >= 0xac00 && code <= 0xd7a3) ||
    (code >= 0xf900 && code <= 0xfaff) ||
    (code >= 0xfe30 && code <= 0xfe6f) ||
    (code >= 0xff00 && code <= 0xff60) ||
    (code >= 0xffe0 && code <= 0xffe6) ||
    (code >= 0x20000 && code <= 0x3fffd)
  ) {
    return 2;
  }
  return 1;
}

function strWidth(text: string): number {
  let width = 0;
  for (const ch of text) width += codepointWidth(ch.codePointAt(0) ?? 0);
  return width;
}

function truncate(text: string, max: number): string {
  if (strWidth(text) <= max) return text;
  if (max <= 0) return "";
  let out = "";
  let width = 0;
  for (const ch of text) {
    const step = codepointWidth(ch.codePointAt(0) ?? 0);
    if (width + step > max - 1) break;
    out += ch;
    width += step;
  }
  return out + "…";
}

function fill(text: string, width: number): string {
  return text + " ".repeat(Math.max(0, width - strWidth(text)));
}

function filteredRows(): any[] {
  const needle = query().toLowerCase();
  if (form !== "goto" || !needle) return rows;
  const useContext = props.filter === "label+context";
  return rows.filter((row: any) => {
    const haystack = `${row.label} ${useContext ? row.context : row.id}`.toLowerCase();
    return haystack.includes(needle);
  });
}

function clampForward(list: any[], index: number): number {
  if (list.length === 0) return 0;
  let next = Math.max(0, Math.min(list.length - 1, index));
  if (form === "goto" && list[next]?.section) {
    while (next + 1 < list.length && list[next]?.section) next += 1;
  }
  return next;
}

function current(): { list: any[]; index: number } {
  const list = filteredRows();
  return { list, index: clampForward(list, selected()) };
}

function move(step: number): void {
  const { list, index } = current();
  if (list.length === 0) return;
  let next = index + step;
  if (form === "chooser") {
    next = (next + list.length) % list.length;
  } else {
    next = Math.max(0, Math.min(list.length - 1, next));
  }
  selected.set(next);
}

function windowStart(index: number): number {
  return index >= visible ? index + 1 - visible : 0;
}

function close(): void {
  const surface = params.surface;
  if (typeof surface === "string") {
    host.call("closeOverlay", { surface }).catch(() => {});
  }
}

function runAction(action: any): void {
  if (!action || typeof action.name !== "string") return;
  host
    .call("run", { name: action.name, args: action.args ?? {} })
    .catch(() => {});
}

function activate(row?: any): void {
  if (!row || !row.action) return;
  close();
  runAction(row.action);
}

function activateSelected(): void {
  const { list, index } = current();
  activate(list[index]);
}

function maybeCurrent(): any {
  const { list, index } = current();
  return list[index];
}

function quick(index: number): void {
  const { list } = current();
  const row = list[index];
  if (!row) return;
  selected.set(index);
  activate(row);
}

function machineOp(op: string): void {
  const row = maybeCurrent();
  const machine = row?.id ?? "";
  if (op === "add") {
    close();
    runAction({ name: "picker.machine", args: { op: "add" } });
    return;
  }
  if (!machine) return;
  if (op !== "connect") close();
  runAction({ name: "picker.machine", args: { op, machine } });
}

function onScroll(event: any): void {
  lastOffset = event?.offset?.y ?? 0;
  const base = autoScroll < 0 ? 0 : autoScroll;
  if (Math.abs(lastOffset - base) <= CELL_H / 2) return;
  if (wheelTimer) clearTimeout(wheelTimer);
  // 一次滚轮手势在停止后才翻译成一行选中移动。
  wheelTimer = setTimeout(() => {
    wheelTimer = 0;
    const anchor = autoScroll < 0 ? 0 : autoScroll;
    if (Math.abs(lastOffset - anchor) <= CELL_H / 2) return;
    const down = lastOffset > anchor;
    autoScroll = -1;
    move(down ? 1 : -1);
  }, 20);
}

effect(() => {
  const { list, index } = current();
  void list;
  const target = windowStart(index) * rowHeight * CELL_H;
  if (!scroller) {
    autoScroll = target;
    return;
  }
  if (autoScroll < 0 || Math.abs(autoScroll - target) > 1) {
    autoScroll = target;
    scroller.scrollToOffset({ offset: target });
  }
});

// ---- keys ----
const keys: Record<string, () => void> = {};
const plain: string[] = [
  "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m",
  "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
  "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
  "-", "=", "[", "]", "\\", ";", "'", ",", ".", "/", "`",
];
const shifted: [string, string][] = [
  ["!", "1"], ["@", "2"], ["#", "3"], ["$", "4"], ["%", "5"],
  ["^", "6"], ["&", "7"], ["*", "8"], ["(", "9"], [")", "0"],
  ["_", "-"], ["+", "="], ["{", "["], ["}", "]"], ["|", "\\"],
  [":", ";"], ["\"", "'"], ["<", ","], [">", "."], ["?", "/"],
  ["~", "`"],
];

function typeChar(ch: string): void {
  if (form !== "goto") return;
  if (ch === "/") return;
  query.update((value: string) => value + ch);
}

for (const ch of plain) {
  keys[ch] = () => typeChar(ch);
}
for (const letter of "ABCDEFGHIJKLMNOPQRSTUVWXYZ") {
  keys[`shift+${letter.toLowerCase()}`] = () => typeChar(letter);
}
keys["space"] = () => typeChar(" ");
for (const [ch, base] of shifted) {
  keys[`shift+${base}`] = () => typeChar(ch);
}

keys["j"] = () => (form === "goto" ? typeChar("j") : move(1));
keys["k"] = () => (form === "goto" ? typeChar("k") : move(-1));
keys["q"] = () => (form === "chooser" ? close() : typeChar("q"));
keys["up"] = () => move(-1);
keys["down"] = () => move(1);
keys["enter"] = () => activateSelected();
keys["escape"] = () => close();
keys["backspace"] = () => {
  if (form !== "goto") return;
  query.update((value: string) => Array.from(value).slice(0, -1).join(""));
};
if (form === "chooser") {
  for (let digit = 1; digit <= 9; digit += 1) {
    keys[String(digit)] = () => quick(digit - 1);
  }
  if (machines) {
    keys["a"] = () => machineOp("add");
    keys["c"] = () => machineOp("connect");
    keys["r"] = () => machineOp("rename");
    keys["d"] = () => machineOp("remove");
    keys["n"] = () => machineOp("open");
  }
}
keymap.bind(keys, { repeat: true });

// ---- drawing ----
function line(props: {
  col: number;
  row: number;
  width: number;
  text: () => string;
  lines?: number;
  fg?: () => string | undefined;
  bg?: () => string | undefined;
  bold?: boolean;
  onPress?: () => void;
}) {
  return (
    <View
      onPress={props.onPress}
      style={{
        position: "absolute",
        left: (props.col + DEBUG_SHIFT_COL) * CELL_W,
        top: props.row * CELL_H,
        width: props.width * CELL_W,
        height: (props.lines ?? 1) * CELL_H,
      }}
    >
      <Text
        style={{
          color: props.fg?.(),
          backgroundColor: props.bg?.(),
          fontWeight: props.bold ? "bold" : "normal",
        }}
      >
        {props.text()}
      </Text>
    </View>
  );
}

function put(chars: string[], at: number, text: string, limit: number): number {
  let column = at;
  for (const ch of text) {
    const step = codepointWidth(ch.codePointAt(0) ?? 0);
    if (column + step > limit) break;
    chars[column] = ch;
    if (step === 2) chars[column + 1] = "";
    column += step;
  }
  return column;
}

function topBorder(title: string): string {
  const chars: string[] = new Array(cols).fill("─");
  chars[0] = "┌";
  if (cols > 1) chars[cols - 1] = "┐";
  if (title) put(chars, 1, truncate(title, Math.max(0, cols - 2)), cols - 1);
  put(chars, cols - CLOSE.length, CLOSE, cols);
  return chars.join("");
}

function bottomBorder(): string {
  return "└" + "─".repeat(Math.max(0, cols - 2)) + "┘";
}

function frame(title: string) {
  const sides = new Array(Math.max(0, INNER_H)).fill("│").join("\n");
  return [
    line({ col: 0, row: 0, width: cols, text: () => topBorder(title), fg: () => color("accent") }),
    line({ col: 0, row: height - 1, width: cols, text: () => bottomBorder(), fg: () => color("accent") }),
    line({ col: 0, row: 1, width: 1, lines: INNER_H, text: () => sides, fg: () => color("accent") }),
    line({ col: cols - 1, row: 1, width: 1, lines: INNER_H, text: () => sides, fg: () => color("accent") }),
  ];
}

function itemRows() {
  const { list, index } = current();
  const shown: any[] = list.map((row: any, order: number) => ({ row, order }));
  if (DEBUG_SHIFT_COL === 0 && DEBUG_SKIP_ROW > 0) {
    shown.splice(Math.max(0, shown.length - 1), 1);
  }
  const elements = shown.map(({ row, order }: any) => {
    const active = order === index;
    const bg = active ? color("selection_bg") : undefined;
    const fg = active ? color("selection_fg") : color("fg");
    const marker = active ? "▸" : " ";
    const text = fill(
      `${marker} ${order + 1}. ${truncate(row.label, Math.max(0, INNER_W - 5))}`,
      INNER_W,
    );
    return (
      <View
        onPress={() => {
          selected.set(order);
          if (machines) return;
          activate(row);
        }}
        style={{ width: INNER_W * CELL_W, height: rowHeight * CELL_H, backgroundColor: bg }}
      >
        <Text style={{ color: fg, backgroundColor: bg }}>{text}</Text>
        {rowHeight > 1 ? (
          <Text style={{ color: fg, backgroundColor: bg }}>{" ".repeat(INNER_W)}</Text>
        ) : null}
      </View>
    );
  });
  // 补足到可视行数，滚轮有可滚动的余量。
  const contentRows = shown.length * rowHeight;
  const padding = Math.max(0, visible * rowHeight + 2 - contentRows);
  for (let index = 0; index < padding; index += 1) {
    elements.push(
      <View style={{ width: INNER_W * CELL_W, height: CELL_H }}>
        <Text>{" ".repeat(INNER_W)}</Text>
      </View>,
    );
  }
  return elements;
}

function machineButtons() {
  if (!machines) return null;
  const groups: string[][] = props.buttons ?? [];
  return groups.map((labels: string[], group: number) => {
    const chars: string[] = new Array(INNER_W).fill(" ");
    let at = 0;
    for (const label of labels) {
      at = put(chars, at, label, INNER_W) + 1;
    }
    return line({
      col: 1,
      row: height - 3 + group,
      width: INNER_W,
      text: () => chars.join(""),
      fg: () => color("selection_fg"),
      bg: () => color("selection_bg"),
    });
  });
}

function Chooser() {
  const viewport = Math.max(0, INNER_H - (machines ? 2 : 0)) * CELL_H;
  return (
    <View style={{ width: cols * CELL_W, height: height * CELL_H }}>
      {frame(props.title ?? "")}
      <Scroll
        ref={(node: any) => {
          scroller = node;
        }}
        onScroll={onScroll}
        scrollbar={false}
        style={{
          position: "absolute",
          left: CELL_W,
          top: CELL_H,
          width: INNER_W * CELL_W,
          height: Math.max(CELL_H, viewport),
          flexDirection: "column",
        }}
      >
        {() => itemRows()}
      </Scroll>
      {machineButtons()}
    </View>
  );
}

function gotoBody() {
  const { list, index } = current();
  const shown: any[] = list.map((row: any, order: number) => ({ row, order }));
  if (DEBUG_SHIFT_COL === 0 && DEBUG_SKIP_ROW > 0) {
    shown.splice(Math.max(0, shown.length - 1), 1);
  }
  return shown.map(({ row, order }: any) => {
    const active = order === index;
    const left = row.section ? ` ${row.label}` : ` ├─ ${row.label}`;
    const right = row.right ?? "";
    const pad = Math.max(0, INNER_W - strWidth(left) - strWidth(right) - 1);
    const text = fill(left + " ".repeat(pad) + right, INNER_W);
    const bg = active ? color("selection_bg") : undefined;
    const fg = active
      ? color("selection_fg")
      : row.section
        ? color("sidebar_title")
        : color("fg");
    return (
      <View
        onPress={() => activate(row)}
        style={{ width: INNER_W * CELL_W, height: CELL_H, backgroundColor: bg }}
      >
        <Text style={{ color: fg, backgroundColor: bg }}>{text}</Text>
      </View>
    );
  });
}

function countLabel(): string {
  const list = filteredRows();
  return `${list.filter((row: any) => !row.section).length} ${props.countLabel ?? "terminals"} `;
}

function queryLine(): string {
  return fill(" / " + (query() || (props.placeholder ?? "")), INNER_W);
}

function contextText(order: number): string {
  const { list, index } = current();
  const chosen = list[index];
  if (!chosen || !chosen.context) return "";
  return order === 0 ? ` ${chosen.context}` : ` ${chosen.cwd ?? ""}`;
}

function navTitle(left: string, right: string): string {
  const pad = Math.max(0, INNER_W - strWidth(left) - strWidth(right));
  return left + " ".repeat(pad) + right;
}

function tailNodes() {
  const bodyRows = Math.max(0, INNER_H - 5);
  const { list, index } = current();
  const chosen = list[index];
  const count = chosen && chosen.context ? 2 : 0;
  const empty = list.length === 0;
  const bodyLines = Math.min(list.length, bodyRows);
  const tailRow = 1 + 3 + bodyLines + (empty ? 1 : 0);
  const nodes: any[] = [];
  if (empty) {
    nodes.push(
      line({
        col: 1,
        row: 1 + 3,
        width: INNER_W,
        text: () => fill(props.empty ?? "", INNER_W),
        fg: () => color("dim"),
      }),
    );
  }
  for (let order = 0; order < count; order += 1) {
    nodes.push(
      line({
        col: 1,
        row: tailRow + order,
        width: INNER_W,
        text: () => fill(contextText(order), INNER_W),
        fg: () => (order === 0 ? color("fg") : color("dim")),
      }),
    );
  }
  const hintRow = tailRow + count;
  if (hintRow <= height - 2) {
    nodes.push(
      line({
        col: 1,
        row: hintRow,
        width: INNER_W,
        text: () => fill(props.hint ?? "", INNER_W),
        fg: () => color("dim"),
      }),
    );
  }
  return nodes;
}

function Goto() {
  const bodyRows = Math.max(0, INNER_H - 5);
  return (
    <View style={{ width: cols * CELL_W, height: height * CELL_H }}>
      {frame("")}
      {line({
        col: 1,
        row: 1,
        width: INNER_W,
        text: () => navTitle(props.navLeft ?? "", countLabel()),
        fg: () => color("dim"),
      })}
      {line({
        col: 1,
        row: 2,
        width: INNER_W,
        text: () => queryLine(),
        fg: () => color(query() ? "fg" : "dim"),
      })}
      {line({
        col: 0,
        row: 3,
        width: cols,
        text: () => "│" + "─".repeat(INNER_W) + "│",
        fg: () => color("border"),
      })}
      <Scroll
        ref={(node: any) => {
          scroller = node;
        }}
        onScroll={onScroll}
        scrollbar={false}
        style={{
          position: "absolute",
          left: CELL_W,
          top: 4 * CELL_H,
          width: INNER_W * CELL_W,
          height: Math.max(CELL_H, bodyRows * CELL_H),
          flexDirection: "column",
        }}
      >
        {() => gotoBody()}
      </Scroll>
      {() => tailNodes()}
    </View>
  );
}

function App() {
  return form === "goto" ? <Goto /> : <Chooser />;
}

mount(() => <App />);
