import { host, mount, signal, Text, View } from "nsh";

type Space = {
  id: string;
  name?: string;
  machine?: string;
  folder?: string;
  tabs?: number;
};

type Snapshot = {
  revision?: number;
  connection?: string;
  spaces?: Space[];
  focused?: { space?: string };
};

const snapshot = signal<Snapshot>({});
const folded = signal<Record<string, boolean>>({});
const error = signal("");

async function refresh() {
  try {
    snapshot.set(await host.call<Snapshot>("state", { domains: ["spaces", "focused"] }));
    error.set("");
  } catch (err) {
    error.set((err as Error).message);
  }
}

function select(space: Space) {
  host
    .call("run", { name: "space.select", args: { id: space.id } })
    .catch((err: Error) => error.set(err.message));
}

function toggle(space: Space) {
  folded.update((state) => ({ ...state, [space.id]: !state[space.id] }));
}

function SpaceRow(props: { space: () => Space; active: () => boolean }) {
  const space = props.space;
  const isFolded = () => folded()[space().id] === true;
  return (
    <View style={{ gap: 1 }}>
      <View
        onPress={() => select(space())}
        role="button"
        style={{
          flexDirection: "row",
          justifyContent: "space-between",
          paddingHorizontal: 4,
          paddingVertical: 1,
          backgroundColor: props.active() ? "#313244" : undefined,
        }}
      >
        <Text style={{ fontSize: 12, fontWeight: props.active() ? "bold" : "normal" }}>
          {space().name || space().id}
        </Text>
        <Text onPress={() => toggle(space())} style={{ fontSize: 10, color: "#89b4fa" }}>
          {isFolded() ? "▸" : "▾"} {space().tabs ?? 0}
        </Text>
      </View>
      {isFolded() ? null : (
        <Text style={{ fontSize: 10, color: "#6e7781", paddingLeft: 8 }}>
          {space().machine ?? "local"} · {space().folder ?? ""}
        </Text>
      )}
    </View>
  );
}

function App() {
  host.on("app.ready", refresh);
  host.on("state.changed", refresh);
  host.on("focus.changed", refresh);
  refresh();

  const spaces = () => snapshot().spaces ?? [];
  const active = (space: Space) => snapshot().focused?.space === space.id;

  return (
    <View style={{ flex: 1, padding: 4, gap: 3 }}>
      <View style={{ flexDirection: "row", justifyContent: "space-between" }}>
        <Text style={{ fontSize: 12, fontWeight: "bold" }}>Spaces</Text>
        <Text style={{ fontSize: 10, color: "#6e7781" }}>
          {snapshot().connection ?? "offline"}
        </Text>
      </View>
      <View style={{ flex: 1, gap: 2 }}>
        <Each of={spaces()}>
          {(space) => <SpaceRow space={space} active={() => active(space())} />}
        </Each>
      </View>
      {error() ? (
        <Text style={{ fontSize: 10, color: "#f38ba8" }}>{error()}</Text>
      ) : null}
    </View>
  );
}

mount(App);
