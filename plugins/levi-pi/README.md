# @levi-tracker/pi

Pi extension for [Levi](https://github.com/ignition-is-go/levi), the git-aware,
agent-first task tracker.

The package keeps Levi authoritative: every operation executes the `levi` CLI
in Pi's current working directory. It adds:

- native `levi_next`, `levi_list`, `levi_show`, `levi_add`, `levi_task`, and
  `levi_dep` agent tools;
- a `/levi` task picker that can inspect, claim, or begin work;
- `lv-…` task-ID completion in the editor; and
- a footer status for tasks claimed by the current Levi identity.

## Install

Install the `levi` binary first, then install the Pi package:

```sh
cargo install levi
pi install npm:@levi-tracker/pi
```

For a source checkout:

```sh
pi install /path/to/levi/plugins/levi-pi
```

The extension loads harmlessly outside Levi repositories and clears its status
when no Levi event log is present.

## Workflow

Ask Pi to select work, or use the picker:

```text
/levi
```

`levi_next` can atomically select and claim eligible work. `levi_task` preserves
Levi's git-aware semantics: closing defaults to an anchor at `HEAD`, so the
fixing commit must exist before the task is closed.

## Development

From the Levi repository root:

```sh
npm install
npm run check --workspace @levi-tracker/pi
```
