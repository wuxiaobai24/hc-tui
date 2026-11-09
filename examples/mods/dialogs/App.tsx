// Dialogs：内置 Prompt 与参数表单的模块实现。
//
// 几何与内置绘制同构（`src/ui/mod.rs:1412` 的 `render_form` 与 `:1651` 的
// `render_prompt`）：宿主把表面矩形开成内置对话框的框体本身，模块只画框内
// 像素。字段行表由宿主按 schema 展开后放进 props，模块负责编辑、增删数组、
// 枚举菜单与本地校验；提交经 `run dialogs.submit` 回到宿主，校验与派发仍走
// `manifest::validate_args` 那一条路径。
//
// 粘贴在 nsh 里到不了自绘表面：宿主把粘贴文本放进 `props.changed` 的
// `paste` 键，模块按内置规则插入（表单字段把换行替换成空格）。

import { Clipboard, host, keymap, mount, signal, Text, View } from "nsh";

type Option = { value: any; label: string };
type Field = {
  label: string;
  display: string;
  depth: number;
  keys: string[];
  kind: "group" | "array" | "text" | "enum" | "bool" | "item" | "add";
  type?: string;
  itemType?: string;
  itemOptions?: any[];
  value?: string;
  cursor?: number;
  checked?: boolean;
  selected?: number | null;
  options?: Option[];
  array?: number;
  required?: boolean;
};
type Props = {
  mode?: string;
  kind?: string;
  title?: string;
  description?: string;
  action?: string;
  _target?: any;
  value?: string;
  cursor?: number;
  fields?: Field[];
  history?: string[];
  buttons?: { primary?: string; secondary?: string; close?: string };
  footer?: string;
  paste?: string;
};

const params = (host.params ?? {}) as any;
const initial = (params.props ?? {}) as Props;
const cellW: number = params.cell?.[0] ?? 8;
const cellH: number = params.cell?.[1] ?? 16;
const tokens = signal<Record<string, string>>((params.theme?.tokens ?? {}) as any);

const mode = initial.mode ?? "prompt";
const title = initial.title ?? "";
const description = initial.description ?? "";
const action = initial.action ?? "";
const target = initial._target ?? null;
const history = (initial.history ?? []).slice();
const buttons = {
  primary: initial.buttons?.primary ?? " Save ",
  secondary: initial.buttons?.secondary ?? " Cancel ",
  close: initial.buttons?.close ?? " [x] ",
};
const footer =
  initial.footer ?? "↵ submit    ⇥ next    esc cancel    ^a add    ^d remove";

const fields = signal<Field[]>(copyFields(initial.fields ?? []));
const selected = signal(firstFocusable(fields()));
const error = signal("");
const errorRow = signal(-1);
const menu = signal<{ row: number; index: number } | null>(null);
const historyPos = signal<number | null>(null);
const size = signal({ cells: 0, rows: 0 });

function copyFields(list: Field[]): Field[] {
  return list.map((field) => ({ ...field, keys: (field.keys ?? []).slice() }));
}

function token(name: string, fallback: string): string {
  return tokens()[name] ?? fallback;
}

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

/// 与内置 `truncate` 同一算法：超宽时截到 `max-1` 列再补 `…`。
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

/// 内置 Paragraph 直接裁掉超宽列，不加省略号。
function clip(text: string, max: number): string {
  if (max <= 0) return "";
  let out = "";
  let width = 0;
  for (const ch of text) {
    const step = codepointWidth(ch.codePointAt(0) ?? 0);
    if (width + step > max) break;
    out += ch;
    width += step;
  }
  return out;
}

function pad(text: string, width: number): string {
  return text + " ".repeat(Math.max(0, width - strWidth(text)));
}

function firstFocusable(list: Field[]): number {
  const index = list.findIndex((field) => field.kind != "group" && field.kind != "array");
  return index < 0 ? 0 : index;
}

function focusable(field: Field | undefined): boolean {
  return !!field && field.kind != "group" && field.kind != "array";
}

function nextFocus(from: number, forward: boolean): number {
  const list = fields();
  if (list.length == 0) return 0;
  let index = from;
  for (let step = 0; step < list.length; step += 1) {
    index = forward
      ? (index + 1) % list.length
      : index == 0
        ? list.length - 1
        : index - 1;
    if (focusable(list[index])) return index;
  }
  return from;
}

function current(): Field | undefined {
  return fields()[selected()];
}

/// 编辑焦点字段的文本；光标按 UTF-16 下标维护，提交时只回值。
function edit(fn: (value: string, cursor: number) => [string, number] | null): void {
  const list = fields();
  const index = selected();
  const field = list[index];
  if (!field || !(field.kind == "text" || field.kind == "item")) return;
  const value = field.value ?? "";
  const cursor = Math.min(field.cursor ?? value.length, value.length);
  const next = fn(value, cursor);
  if (!next) return;
  list[index] = { ...field, value: next[0], cursor: next[1] };
  fields.set(list.slice());
}

function insertText(text: string): void {
  if (mode == "prompt") {
    const text2 = text;
    editPrompt((value, cursor) => [
      value.slice(0, cursor) + text2 + value.slice(cursor),
      cursor + text2.length,
    ]);
    return;
  }
  const list = fields();
  const index = selected();
  const field = list[index];
  if (!field || !(field.kind == "text" || field.kind == "item")) return;
  const text2 = text.replace(/[\r\n]/g, " ");
  edit((value, cursor) => [
    value.slice(0, cursor) + text2 + value.slice(cursor),
    cursor + text2.length,
  ]);
}

function editPrompt(
  fn: (value: string, cursor: number) => [string, number] | null,
): void {
  const list = fields();
  const field = list[0];
  if (!field) return;
  const value = field.value ?? "";
  const cursor = Math.min(field.cursor ?? value.length, value.length);
  const next = fn(value, cursor);
  if (!next) return;
  list[0] = { ...field, value: next[0], cursor: next[1] };
  fields.set(list.slice());
  historyPos.set(null);
}

function typeChar(ch: string): void {
  if (mode == "prompt") {
    editPrompt((value, cursor) => [
      value.slice(0, cursor) + ch + value.slice(cursor),
      cursor + ch.length,
    ]);
    return;
  }
  insertText(ch);
}

function backspace(): void {
  const editFn = (value: string, cursor: number): [string, number] | null => {
    if (cursor == 0) return null;
    const word = Array.from(value.slice(0, cursor));
    word.pop();
    const before = word.join("");
    return [before + value.slice(cursor), before.length];
  };
  if (mode == "prompt") editPrompt(editFn);
  else edit(editFn);
}

function deleteForward(): void {
  const editFn = (value: string, cursor: number): [string, number] | null => {
    if (cursor >= value.length) return null;
    const word = Array.from(value.slice(cursor));
    word.shift();
    return [value.slice(0, cursor) + word.join(""), cursor];
  };
  if (mode == "prompt") editPrompt(editFn);
  else edit(editFn);
}

function moveCursor(delta: number): void {
  const move = (value: string, cursor: number): [string, number] | null => {
    if (delta < 0) {
      if (cursor == 0) return null;
      const before = Array.from(value.slice(0, cursor));
      before.pop();
      return [value, before.join("").length];
    }
    if (cursor >= value.length) return null;
    const step = Array.from(value.slice(cursor))[0] ?? "";
    return [value, cursor + step.length];
  };
  if (mode == "prompt") editPrompt(move);
  else edit(move);
}

function historyMove(delta: number): void {
  if (mode != "prompt" || history.length == 0) return;
  const list = fields();
  const field = list[0];
  if (!field) return;
  const position = historyPos();
  let next: number | null = position;
  let value = field.value ?? "";
  if (delta < 0) {
    if (position == null) next = history.length - 1;
    else if (position > 0) next = position - 1;
    if (next != null) value = history[next];
  } else if (position != null) {
    const candidate = position + 1;
    if (candidate < history.length) {
      next = candidate;
      value = history[candidate];
    } else {
      next = null;
      value = "";
    }
  }
  historyPos.set(next);
  list[0] = { ...field, value, cursor: value.length };
  fields.set(list.slice());
}

function toggleBool(): void {
  const list = fields();
  const field = list[selected()];
  if (!field || field.kind != "bool") return;
  list[selected()] = { ...field, checked: !field.checked };
  fields.set(list.slice());
}

// ---- 数组增删（与 `src/app/form.rs` 的 add_item/remove_item 对齐）----

function reindex(array: number): void {
  const list = fields();
  let index = 0;
  for (let at = 0; at < list.length; at += 1) {
    const field = list[at];
    if (field.kind == "item" && field.array == array) {
      list[at] = {
        ...field,
        label: `[${index}]`,
        display: `${(field.keys ?? []).join(".")}[${index}]`,
      };
      index += 1;
    }
  }
}

function addItem(array: number): void {
  const list = fields();
  const at = list.findIndex((field) => field.kind == "add" && field.array == array);
  if (at < 0) return;
  const add = list[at];
  const group = list.find(
    (field) => field.kind == "array" && (field.keys ?? []).join(".") == add.keys.join("."),
  );
  list.splice(at, 0, {
    label: "[0]",
    display: `${add.keys.join(".")}[0]`,
    depth: add.depth,
    keys: add.keys.slice(),
    kind: "item",
    type: (group?.itemType as string) ?? "string",
    value: "",
    cursor: 0,
    array,
  });
  reindex(array);
  fields.set(list.slice());
  selected.set(at);
  host.call("run", { name: "dialogs.resize", args: { rows: list.length } }).catch(() => {});
}

function removeItem(index: number): void {
  const list = fields();
  const field = list[index];
  if (!field || field.kind != "item" || field.array === undefined) return;
  const array = field.array;
  list.splice(index, 1);
  reindex(array);
  fields.set(list.slice());
  let target = Math.min(index, list.length - 1);
  if (!focusable(list[target])) target = nextFocus(target, true);
  selected.set(Math.max(0, target));
  host.call("run", { name: "dialogs.resize", args: { rows: list.length } }).catch(() => {});
}

// ---- 校验与提交 ----

function scalar(field: Field, text: string): any {
  const kind = field.type ?? "string";
  if (kind == "integer") {
    return /^[+-]?\d+$/.test(text) ? Number(text) : text;
  }
  if (kind == "number") {
    return /^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$/.test(text)
      ? Number(text)
      : text;
  }
  if (kind == "boolean") {
    if (text == "true") return true;
    if (text == "false") return false;
    return text;
  }
  return text;
}

function put(root: any, keys: string[], value: any): void {
  let node = root;
  for (let index = 0; index < keys.length; index += 1) {
    const key = keys[index];
    if (index == keys.length - 1) {
      node[key] = value;
      return;
    }
    if (typeof node[key] != "object" || node[key] == null) node[key] = {};
    node = node[key];
  }
}

function buildValues(): { values: any; error?: string; row?: number } {
  const list = fields();
  const values: any = {};
  const arrays: Record<number, any[]> = {};
  let problem: { error: string; row: number } | null = null;
  for (let index = 0; index < list.length; index += 1) {
    const field = list[index];
    if (field.kind == "item") {
      const id = field.array ?? 0;
      const text = field.value ?? "";
      const value = scalar(field, text);
      if (value === text && (field.type == "integer" || field.type == "number" || field.type == "boolean")) {
        if (!problem) {
          problem = { error: `args.${field.display} must be ${field.type}`, row: index };
        }
      }
      (arrays[id] ??= []).push(value);
      continue;
    }
    if (field.kind == "array") continue;
    if (field.kind == "group") {
      put(values, field.keys, {});
      continue;
    }
    if (field.kind == "bool") {
      put(values, field.keys, !!field.checked);
      continue;
    }
    if (field.kind == "enum") {
      const options = field.options ?? [];
      const chosen = field.selected;
      if (chosen == null) {
        if (field.required && !problem) {
          problem = { error: `args.${field.display} is required`, row: index };
        }
        continue;
      }
      put(values, field.keys, options[chosen]?.value);
      continue;
    }
    if (field.kind == "add") continue;
    const text = field.value ?? "";
    if (text.length == 0) {
      if (field.required && !problem) {
        problem = { error: `args.${field.display} is required`, row: index };
      }
      continue;
    }
    const value = scalar(field, text);
    if (value === text && (field.type == "integer" || field.type == "number" || field.type == "boolean")) {
      if (!problem) {
        problem = { error: `args.${field.display} must be ${field.type}`, row: index };
      }
    }
    put(values, field.keys, value);
  }
  for (const field of list) {
    if (field.kind != "array") continue;
    put(values, field.keys, arrays[field.array ?? 0] ?? []);
  }
  return { values, ...(problem ?? {}) };
}

function focusError(message: string): void {
  const tokenText = message.split(/\s+/)[0] ?? "";
  const path = tokenText.startsWith("args.") ? tokenText.slice(5) : tokenText;
  let best = -1;
  let bestLength = -1;
  fields().forEach((field, index) => {
    if (!focusable(field) || !field.display) return;
    const inside =
      path == field.display ||
      path.startsWith(`${field.display}.`) ||
      path.startsWith(`${field.display}[`);
    if (inside && field.display.length > bestLength) {
      best = index;
      bestLength = field.display.length;
    }
  });
  if (best >= 0) selected.set(best);
  errorRow.set(best);
}

function showError(message: string): void {
  error.set(message);
  focusError(message);
}

function closeSurface(): void {
  const surface = params.surface;
  if (typeof surface == "string") {
    host.call("closeOverlay", { surface }).catch(() => {});
  }
}

async function submit(): Promise<void> {
  const list = fields();
  const field = list[selected()];
  if (menu()) return;
  if (mode == "prompt") {
    const value = list[0]?.value ?? "";
    try {
      const reply: any = await host.call("run", {
        name: "dialogs.submit",
        args: { value },
      });
      if (reply?.accepted === false) {
        showError(reply?.error ?? "rejected");
        return;
      }
      closeSurface();
    } catch (err) {
      showError((err as Error).message);
    }
    return;
  }
  if (field?.kind == "add") {
    addItem(field.array ?? 0);
    return;
  }
  if (field?.kind == "enum") {
    menu.set({ row: selected(), index: field.selected ?? 0 });
    return;
  }
  const built = buildValues();
  if (built.error) {
    showError(built.error);
    return;
  }
  error.set("");
  errorRow.set(-1);
  try {
    const reply: any = await host.call("run", {
      name: "dialogs.submit",
      args: { values: built.values, _target: target },
    });
    if (reply?.accepted === false) {
      showError(reply?.error ?? "rejected");
      return;
    }
    closeSurface();
  } catch (err) {
    showError((err as Error).message);
  }
}

function cancel(): void {
  host.call("run", { name: "dialogs.cancel", args: {} }).catch(() => {});
  closeSurface();
}

// ---- 按键 ----

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

for (const ch of plain) keys[ch] = () => typeChar(ch);
for (const letter of "ABCDEFGHIJKLMNOPQRSTUVWXYZ") {
  keys[`shift+${letter.toLowerCase()}`] = () => typeChar(letter);
}
keys["space"] = () => {
  if (mode == "form" && current()?.kind == "bool") toggleBool();
  else typeChar(" ");
};
for (const [ch, base] of shifted) keys[`shift+${base}`] = () => typeChar(ch);
keys["backspace"] = () => backspace();
keys["delete"] = () => deleteForward();
keys["left"] = () => {
  if (mode == "form" && current()?.kind == "bool") toggleBool();
  else moveCursor(-1);
};
keys["right"] = () => {
  if (mode == "form" && current()?.kind == "bool") toggleBool();
  else moveCursor(1);
};
keys["home"] = () => {
  if (mode == "prompt") editPrompt((value) => [value, 0]);
  else edit((value) => [value, 0]);
};
keys["end"] = () =>
  mode == "prompt"
    ? editPrompt((value) => [value, value.length])
    : edit((value) => [value, value.length]);
keys["up"] = () => {
  if (menu()) {
    moveMenu(-1);
    return;
  }
  if (mode == "prompt") historyMove(-1);
  else selected.set(nextFocus(selected(), false));
};
keys["down"] = () => {
  if (menu()) {
    moveMenu(1);
    return;
  }
  if (mode == "prompt") historyMove(1);
  else selected.set(nextFocus(selected(), true));
};
keys["tab"] = () => {
  if (menu()) {
    menu.set(null);
    return;
  }
  if (mode == "form") selected.set(nextFocus(selected(), true));
};
keys["shift+tab"] = () => {
  if (menu()) {
    menu.set(null);
    return;
  }
  if (mode == "form") selected.set(nextFocus(selected(), false));
};
keys["enter"] = () => {
  const open = menu();
  if (open) {
    chooseMenu(open);
    return;
  }
  void submit();
};
keys["escape"] = () => {
  if (menu()) {
    menu.set(null);
    return;
  }
  cancel();
};
keys["ctrl+c"] = () => {
  if (mode == "prompt") editPrompt(() => ["", 0]);
};
keys["ctrl+u"] = () => {
  if (mode == "prompt") editPrompt((value) => ["", 0]);
  else edit((value) => ["", 0]);
};
keys["ctrl+a"] = () => {
  const field = current();
  if (mode == "form" && field && (field.kind == "item" || field.kind == "add")) {
    addItem(field.array ?? 0);
  }
};
keys["ctrl+d"] = () => {
  const field = current();
  if (mode == "form" && field?.kind == "item") removeItem(selected());
};
keys["ctrl+v"] = () => {
  Clipboard.readText()
    .then((text: string | null) => {
      if (text) insertText(text);
    })
    .catch(() => {});
};
keymap.bind(keys, { repeat: true });

function moveMenu(delta: number): void {
  const open = menu();
  if (!open) return;
  const field = fields()[open.row];
  const count = field?.options?.length ?? 0;
  if (count == 0) return;
  menu.set({ row: open.row, index: Math.max(0, Math.min(count - 1, open.index + delta)) });
}

function chooseMenu(open: { row: number; index: number }): void {
  const list = fields();
  const field = list[open.row];
  if (!field) {
    menu.set(null);
    return;
  }
  list[open.row] = { ...field, selected: open.index };
  fields.set(list.slice());
  menu.set(null);
}

// ---- 绘制 ----

function promptRows(cells: number, rows: number): { text: string; color: string }[] {
  const inner = cells - 2;
  const out: { text: string; color: string }[] = [];
  const field = fields()[0];
  const value = field?.value ?? "";
  const cursor = Math.min(field?.cursor ?? value.length, value.length);
  const before = value.slice(0, cursor);
  const after = value.slice(cursor);
  const primary = buttons.primary;
  const offset = Math.max(Math.floor(inner / 2), 10);
  let choice = " ".repeat(inner);
  choice = clip(primary, inner) + choice.slice(primary.length);
  choice = pad(choice.slice(0, offset), offset);
  choice += clip(buttons.secondary, inner - offset);
  choice = pad(choice, inner);
  const body = [
    clip(` ${title}`, inner),
    truncate(description, inner),
    clip(` ${before}▏${after}`, inner),
    choice,
    "",
  ];
  out.push({
    text: `┌${"─".repeat(Math.max(0, cells - 6))}${buttons.close}`,
    color: token("accent", "#89b4fa"),
  });
  for (const line of body) {
    out.push({ text: `│${pad(line, inner)}│`, color: token("fg", "#cdd6f4") });
  }
  out.push({
    text: `└${"─".repeat(Math.max(0, cells - 2))}┘`,
    color: token("accent", "#89b4fa"),
  });
  while (out.length < rows) out.push({ text: " ".repeat(cells), color: token("fg", "#cdd6f4") });
  return out;
}

/// 一行字段；逐字符复刻 `form_row_line`。
function fieldLine(field: Field, focused: boolean): string {
  const indent = `  ${"  ".repeat(field.depth)}`;
  switch (field.kind) {
    case "group":
    case "array":
      return indent + field.label;
    case "bool":
      return `${indent}${field.label}: ${field.checked ? "[x] on" : "[ ] off"}`;
    case "enum": {
      const options = field.options ?? [];
      const chosen = field.selected;
      const text = chosen == null ? "—" : (options[chosen]?.label ?? "—");
      return `${indent}${field.label}: ${text}  ◂ ▸`;
    }
    case "add":
      return indent + field.label;
    case "item":
    case "text": {
      const label = field.label ? `${field.label}: ` : "";
      if (focused) {
        const value = field.value ?? "";
        const cursor = Math.min(field.cursor ?? value.length, value.length);
        return `${indent}${label}${value.slice(0, cursor)}▏${value.slice(cursor)}`;
      }
      const value = field.value ?? "";
      return `${indent}${label}${value.length == 0 ? "—" : value}`;
    }
  }
  return "";
}

function formRows(cells: number, rows: number): { text: string; color: string }[] {
  const inner = cells - 2;
  const list = fields();
  const fieldHeight = Math.max(0, rows - 5);
  const lines: { text: string; color: string; row: number }[] = [];
  list.forEach((field, index) => {
    lines.push({
      text: fieldLine(field, index == selected()),
      color:
        errorRow() == index
          ? token("agent_error", "#f38ba8")
          : index == selected()
            ? token("accent", "#89b4fa")
            : token("fg", "#cdd6f4"),
      row: index,
    });
    if (errorRow() == index && error()) {
      lines.push({
        text: `  ${"  ".repeat(field.depth + 1)}${truncate(error(), inner - 4)}`,
        color: token("agent_error", "#f38ba8"),
        row: -1,
      });
    }
  });
  let scroll = 0;
  const selectedLine = Math.max(
    0,
    lines.findIndex((line) => line.row == selected()),
  );
  if (fieldHeight > 0) {
    scroll = Math.max(0, selectedLine + 1 - fieldHeight);
  }
  scroll = Math.min(scroll, Math.max(0, lines.length - fieldHeight));
  const visible = lines.slice(scroll, scroll + fieldHeight);
  const out: { text: string; color: string }[] = [];
  out.push({
    text: `┌${"─".repeat(Math.max(0, cells - 2))}┐`,
    color: token("accent", "#89b4fa"),
  });
  out.push({
    text: `│${pad(clip(` ${title}  ${action}`, inner), inner)}│`,
    color: token("accent", "#89b4fa"),
  });
  for (let index = 0; index < fieldHeight; index += 1) {
    const line = visible[index];
    out.push({
      text: `│${pad(clip(line?.text ?? "", inner), inner)}│`,
      color: line?.color ?? token("fg", "#cdd6f4"),
    });
  }
  const errorLine =
    errorRow() < 0 && error() ? clip(error(), inner) : "";
  out.push({
    text: `│${pad(errorLine, inner)}│`,
    color: token("agent_error", "#f38ba8"),
  });
  out.push({
    text: `│${pad(clip(footer, inner), inner)}│`,
    color: token("dim", "#7f849c"),
  });
  out.push({
    text: `└${"─".repeat(Math.max(0, cells - 2))}┘`,
    color: token("accent", "#89b4fa"),
  });
  while (out.length < rows) {
    out.push({ text: " ".repeat(cells), color: token("fg", "#cdd6f4") });
  }
  if (menu()) {
    drawMenu(out, cells, rows);
  }
  return out;
}

/// 枚举菜单：绘制在表单框内部（内置实现画在内容区中心，模块画不到表面外）。
function drawMenu(out: { text: string; color: string }[], cells: number, rows: number): void {
  const open = menu();
  if (!open) return;
  const field = fields()[open.row];
  const options = field?.options ?? [];
  if (options.length == 0) return;
  const width = Math.min(40, Math.max(8, cells - 4));
  const height = Math.min(options.length + 2, Math.max(3, rows - 2));
  const x = Math.floor((cells - width) / 2);
  const y = Math.max(1, Math.floor((rows - height) / 2));
  const put = (row: number, text: string) => {
    if (row < 0 || row >= out.length) return;
    const current = out[row].text;
    out[row] = {
      text: current.slice(0, x) + clip(text, width) + current.slice(x + width),
      color: token("fg", "#cdd6f4"),
    };
  };
  put(y, `┌${"─".repeat(width - 2)}┐`);
  for (let index = 0; index < height - 2; index += 1) {
    const option = options[Math.min(index, options.length - 1)];
    const marker = index == open.index ? "▸" : " ";
    put(y + 1 + index, `│${pad(clip(`${marker} ${option?.label ?? ""}`, width - 2), width - 2)}│`);
  }
  put(y + height - 1, `└${"─".repeat(width - 2)}┘`);
}

function App() {
  host.on("theme.changed", (theme: any) => {
    if (theme?.tokens) tokens.set(theme.tokens);
  });
  host.on("props.changed", (payload: any) => {
    const next = payload?.props as Props | undefined;
    if (!next) return;
    if (typeof next.paste == "string") insertText(next.paste);
  });

  const rows = () => {
    const cells = size().cells;
    const count = size().rows;
    if (cells < 20 || count < 5) return [];
    return mode == "prompt" ? promptRows(cells, count) : formRows(cells, count);
  };
  const background = () => token("sidebar_bg", "#1e1e2e");

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
      style={{ flex: 1, flexDirection: "column", backgroundColor: background() }}
    >
      <Each of={rows()}>
        {(row) => (
          <View style={{ height: cellH, flexDirection: "row", backgroundColor: background() }}>
            <Text style={{ color: row().color, backgroundColor: background() }}>{row().text}</Text>
          </View>
        )}
      </Each>
    </View>
  );
}

mount(App);
