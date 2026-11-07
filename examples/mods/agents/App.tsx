import { host, mount, signal, Text, View } from "nsh";

type Agent = {
  address: string;
  name?: string;
  title?: string;
  cwd?: string;
  turn?: number;
  pinned?: boolean;
};

type Focused = { space?: string; tab?: string; pane?: string; mod?: string };

type Snapshot = {
  revision?: number;
  connection?: string;
  agents?: Agent[];
  focused?: Focused;
};

const snapshot = signal<Snapshot>({});
const error = signal("");

async function refresh() {
  try {
    snapshot.set(
      await host.call<Snapshot>("state", { domains: ["agents", "focused"] }),
    );
    error.set("");
  } catch (err) {
    error.set((err as Error).message);
  }
}

function focus(agent: Agent) {
  host
    .call("run", { name: "agent.focus", args: { address: agent.address } })
    .catch((err: Error) => error.set(err.message));
}

function label(agent: Agent) {
  return agent.name || agent.title || agent.address;
}

function Row(props: { agent: () => Agent; current: () => boolean }) {
  const agent = props.agent;
  return (
    <View
      onPress={() => focus(agent())}
      role="button"
      style={{
        paddingHorizontal: 4,
        paddingVertical: 1,
        backgroundColor: props.current() ? "#313244" : undefined,
      }}
    >
      <Text style={{ fontSize: 12, fontWeight: props.current() ? "bold" : "normal" }}>
        {agent().pinned ? "★ " : ""}
        {label(agent())}
      </Text>
      <Text style={{ fontSize: 10, color: "#6e7781" }}>
        {agent().cwd ?? agent().address}
      </Text>
    </View>
  );
}

function App() {
  host.on("app.ready", refresh);
  host.on("state.changed", refresh);
  host.on("focus.changed", refresh);
  refresh();

  const agents = () => snapshot().agents ?? [];
  const current = (agent: Agent) => {
    const pane = snapshot().focused?.pane;
    return pane !== undefined && agent.address.endsWith(`/${pane}`);
  };

  return (
    <View style={{ flex: 1, padding: 4, gap: 2 }}>
      <View style={{ flexDirection: "row", justifyContent: "space-between" }}>
        <Text style={{ fontSize: 12, fontWeight: "bold" }}>Agents</Text>
        <Text style={{ fontSize: 10, color: "#6e7781" }}>
          {snapshot().connection ?? "offline"} · {agents().length}
        </Text>
      </View>
      <View onPress={refresh} role="button" style={{ paddingVertical: 1 }}>
        <Text style={{ fontSize: 10, color: "#89b4fa" }}>Refresh</Text>
      </View>
      <View style={{ flex: 1, gap: 1 }}>
        <Each of={agents()}>
          {(agent) => <Row agent={agent} current={() => current(agent())} />}
        </Each>
      </View>
      {error() ? (
        <Text style={{ fontSize: 10, color: "#f38ba8" }}>{error()}</Text>
      ) : null}
    </View>
  );
}

mount(App);
