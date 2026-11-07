import { host, mount, signal, Text, View } from "nsh";

type PullRequest = {
  number: number;
  title: string;
  state?: string;
  author?: { login?: string };
};

type ExecResult = {
  code: number | null;
  stdout: string;
  stderr: string;
  timed_out: boolean;
  truncated: boolean;
};

const rows = signal<PullRequest[]>([]);
const error = signal("");
const loading = signal(false);

async function load() {
  loading.set(true);
  error.set("");
  try {
    const result = await host.call<ExecResult>("exec", {
      argv: ["gh", "pr", "list", "--json", "number,title,state,author", "--limit", "50"],
      timeout_s: 30,
    });
    if (result.timed_out) {
      error.set("gh timed out");
    } else if (result.code !== 0) {
      error.set(result.stderr.trim() || `gh exited with ${result.code}`);
    } else {
      rows.set(JSON.parse(result.stdout) as PullRequest[]);
    }
  } catch (err) {
    error.set((err as Error).message);
  } finally {
    loading.set(false);
  }
}

function close() {
  const surface = host.params.surface;
  if (typeof surface === "string") {
    host.call("closeOverlay", { surface }).catch(() => {});
  }
}

function App() {
  load();
  host.on("app.ready", load);

  return (
    <View style={{ flex: 1, padding: 8, gap: 4 }}>
      <View style={{ flexDirection: "row", justifyContent: "space-between" }}>
        <Text style={{ fontSize: 14, fontWeight: "bold" }}>Pull requests</Text>
        <Text style={{ fontSize: 10, color: "#89b4fa" }} onPress={load}>
          {loading() ? "loading…" : "reload"}
        </Text>
      </View>
      <View style={{ flex: 1, gap: 1 }}>
        <Each of={rows()}>
          {(pr) => (
            <View style={{ flexDirection: "row", gap: 2 }}>
              <Text style={{ fontSize: 12, color: "#89b4fa" }}>#{pr().number}</Text>
              <Text style={{ fontSize: 12 }}>{pr().title}</Text>
              <Text style={{ fontSize: 10, color: "#6e7781" }}>
                {pr().state ?? ""} {pr().author?.login ?? ""}
              </Text>
            </View>
          )}
        </Each>
      </View>
      {error() ? <Text style={{ fontSize: 10, color: "#f38ba8" }}>{error()}</Text> : null}
      <Text onPress={close} style={{ fontSize: 10, color: "#6e7781" }}>
        Esc closes this dialog
      </Text>
    </View>
  );
}

mount(App);
