// Git board：一列最近提交。与 examples/mods/* 的 dist/bundle.js 同为
// `nsh build` 产物形态；这里直接维护源码，免去安装示例前的构建步骤。
import { append, each, element, insert, prop, text } from "nsh:rt";
import { host, mount, signal } from "nsh";

const commits = signal([]);
const error = signal("");

async function refresh() {
  try {
    const result = await host.call("exec", {
      argv: ["git", "log", "--oneline", "--decorate", "-12"],
      timeout_s: 10,
    });
    const output = String(result.stdout || "").trim();
    commits.set(output ? output.split("\n") : []);
    error.set("");
  } catch (err) {
    commits.set([]);
    error.set(err.message);
  }
}

host.on("state.changed", refresh);
host.on("grant.changed", refresh);

function App() {
  const view = element("view");
  prop(view, "style", { paddingHorizontal: 2, paddingVertical: 1, gap: 1 });

  const title = element("text");
  prop(title, "style", { fontWeight: "bold", fontSize: 12 });
  append(title, text("Git board"));
  append(view, title);

  const subtitle = element("text");
  prop(subtitle, "style", { fontSize: 10, color: "#6e7781" });
  append(subtitle, text("recent commits"));
  append(view, subtitle);

  const list = element("view");
  prop(list, "style", { flex: 1, gap: 1 });
  insert(
    list,
    each(
      () => commits(),
      (line) => {
        const row = element("text");
        prop(row, "style", { fontSize: 11 });
        append(row, text(line()));
        return row;
      },
    ),
  );
  append(view, list);

  const button = element("view");
  prop(button, "onPress", refresh);
  prop(button, "role", "button");
  prop(button, "style", { paddingVertical: 1 });
  const label = element("text");
  prop(label, "style", { fontSize: 10, color: "#89b4fa" });
  append(label, text("Refresh"));
  append(button, label);
  append(view, button);

  insert(view, () => {
    if (!error()) {
      return null;
    }
    const line = element("text");
    prop(line, "style", { fontSize: 10, color: "#f38ba8" });
    append(line, text(error()));
    return line;
  });
  return view;
}

mount(App);
refresh();
