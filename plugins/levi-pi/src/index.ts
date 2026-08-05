import { StringEnum } from "@earendil-works/pi-ai";
import {
  DEFAULT_MAX_BYTES,
  DEFAULT_MAX_LINES,
  formatSize,
  truncateHead,
  type ExtensionAPI,
  type ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import type { AutocompleteItem } from "@earendil-works/pi-tui";
import { Type } from "typebox";

interface Claim {
  dev: string;
  machine: string;
  worktree: string;
}

interface Task {
  id: string;
  short: string;
  title: string;
  priority: string;
  status: string;
  labels: string[];
  claim: Claim | null;
  reason?: string;
}

interface TaskList {
  schema: "levi.ls/1" | "levi.next/1";
  tasks: Task[];
}

interface CommandResult {
  stdout: string;
  stderr: string;
}

const STATUS_KEY = "levi";
const JSON_TIMEOUT_MS = 30_000;
const CACHE_MS = 5_000;

function errorText(result: { code: number; stdout: string; stderr: string }): string {
  return result.stderr.trim() || result.stdout.trim() || `levi exited with code ${result.code}`;
}

function toolResult(stdout: string, stderr: string, command: string[]) {
  const truncated = truncateHead(stdout.trim() || "ok", {
    maxBytes: DEFAULT_MAX_BYTES,
    maxLines: DEFAULT_MAX_LINES,
  });
  let text = truncated.content;
  if (truncated.truncated) {
    text += `\n\n[Output truncated to ${truncated.outputLines} lines (${formatSize(truncated.outputBytes)}).]`;
  }
  if (stderr.trim()) text += `\n\nWarnings:\n${stderr.trim()}`;
  return {
    content: [{ type: "text" as const, text }],
    details: { command: ["levi", ...command], truncated: truncated.truncated },
  };
}

function taskLabel(task: Task): string {
  const claimed = task.claim ? ` · claimed by ${task.claim.dev}` : "";
  return `${task.short} · ${task.priority} · ${task.title}${claimed}`;
}

export default function leviPi(pi: ExtensionAPI): void {
  let active = false;
  let cachedTasks: { at: number; tasks: Task[] } | undefined;

  async function run(
    cwd: string,
    args: string[],
    signal?: AbortSignal,
  ): Promise<CommandResult> {
    const result = await pi.exec("levi", args, {
      cwd,
      signal,
      timeout: JSON_TIMEOUT_MS,
    });
    if (result.code !== 0) throw new Error(errorText(result));
    return { stdout: result.stdout, stderr: result.stderr };
  }

  async function json<T>(
    cwd: string,
    args: string[],
    signal?: AbortSignal,
  ): Promise<{ data: T; result: CommandResult }> {
    const result = await run(cwd, args, signal);
    try {
      return { data: JSON.parse(result.stdout) as T, result };
    } catch {
      throw new Error(`levi returned invalid JSON for ${args.join(" ")}`);
    }
  }

  async function allTasks(cwd: string, signal?: AbortSignal): Promise<Task[]> {
    if (cachedTasks && Date.now() - cachedTasks.at < CACHE_MS) return cachedTasks.tasks;
    const { data } = await json<TaskList>(cwd, ["ls", "--all", "--json"], signal);
    cachedTasks = { at: Date.now(), tasks: data.tasks };
    return data.tasks;
  }

  async function refreshStatus(ctx: ExtensionContext): Promise<void> {
    try {
      const { data, result } = await json<TaskList>(ctx.cwd, ["ls", "--mine", "--json"]);
      active = !result.stderr.includes("no levi events here");
      if (!active) {
        ctx.ui.setStatus(STATUS_KEY, undefined);
        return;
      }
      const claims = data.tasks;
      if (claims.length === 0) {
        ctx.ui.setStatus(STATUS_KEY, undefined);
      } else if (claims.length === 1) {
        ctx.ui.setStatus(STATUS_KEY, `levi: ${claims[0]!.short} · ${claims[0]!.title}`);
      } else {
        ctx.ui.setStatus(STATUS_KEY, `levi: ${claims.length} claimed tasks`);
      }
    } catch {
      active = false;
      ctx.ui.setStatus(STATUS_KEY, undefined);
    }
  }

  function changed(ctx: ExtensionContext): void {
    cachedTasks = undefined;
    void refreshStatus(ctx);
  }

  pi.registerTool({
    name: "levi_next",
    label: "Levi Next",
    description:
      "Return Levi's deterministically highest-ranked eligible tasks. With claim=true, atomically claim the top task for this developer/machine/worktree.",
    promptSnippet: "Select and atomically claim the next eligible Levi task",
    promptGuidelines: [
      "Use levi_next with claim=true when selecting unspecified work from a Levi-managed repository.",
    ],
    parameters: Type.Object({
      claim: Type.Optional(Type.Boolean({ description: "Atomically claim the top task" })),
      count: Type.Optional(Type.Integer({ minimum: 1, maximum: 20, description: "Number of ranked tasks" })),
    }),
    async execute(_id, params, signal, _update, ctx) {
      const args = ["next", "--json", "--count", String(params.count ?? 1)];
      if (params.claim) args.push("--claim");
      const result = await run(ctx.cwd, args, signal);
      if (params.claim) changed(ctx);
      return toolResult(result.stdout, result.stderr, args);
    },
  });

  pi.registerTool({
    name: "levi_list",
    label: "Levi List",
    description: "List Levi tasks using its stable levi.ls/1 JSON schema.",
    promptSnippet: "List open, closed, claimed, or labelled Levi tasks",
    parameters: Type.Object({
      state: Type.Optional(StringEnum(["open", "closed", "all"] as const)),
      mine: Type.Optional(Type.Boolean({ description: "Only tasks claimed by this identity" })),
      label: Type.Optional(Type.String()),
      branch: Type.Optional(Type.String({ description: "Resolve against this local branch instead of HEAD" })),
    }),
    async execute(_id, params, signal, _update, ctx) {
      const args = ["ls", "--json"];
      if (params.state === "closed") args.push("--closed");
      if (params.state === "all") args.push("--all");
      if (params.mine) args.push("--mine");
      if (params.label) args.push("--label", params.label);
      if (params.branch) args.push("--branch", params.branch);
      const result = await run(ctx.cwd, args, signal);
      return toolResult(result.stdout, result.stderr, args);
    },
  });

  pi.registerTool({
    name: "levi_show",
    label: "Levi Show",
    description: "Show a Levi task including its body, dependencies, claim, comments, and status history.",
    promptSnippet: "Inspect a Levi task and its dependencies/history",
    parameters: Type.Object({ id: Type.String({ description: "Task ID or unambiguous prefix" }) }),
    async execute(_id, params, signal, _update, ctx) {
      const args = ["show", params.id, "--json"];
      const result = await run(ctx.cwd, args, signal);
      return toolResult(result.stdout, result.stderr, args);
    },
  });

  pi.registerTool({
    name: "levi_add",
    label: "Levi Add",
    description: "Create a Levi task locally or in another hub-connected project.",
    promptSnippet: "Create a local or cross-project Levi task",
    parameters: Type.Object({
      title: Type.String(),
      priority: Type.Optional(StringEnum(["p0", "p1", "p2", "p3"] as const)),
      body: Type.Optional(Type.String()),
      labels: Type.Optional(Type.Array(Type.String())),
      dependencies: Type.Optional(Type.Array(Type.String({ description: "Local blocker task IDs" }))),
      project: Type.Optional(Type.String({ description: "Foreign project name or ID" })),
    }),
    async execute(_id, params, signal, _update, ctx) {
      const args = ["add", params.title, "--json"];
      if (params.priority) args.push("--priority", params.priority);
      if (params.body) args.push("--body", params.body);
      for (const label of params.labels ?? []) args.push("--label", label);
      for (const dependency of params.dependencies ?? []) args.push("--dep", dependency);
      if (params.project) args.push("--project", params.project);
      const result = await run(ctx.cwd, args, signal);
      changed(ctx);
      return toolResult(result.stdout, result.stderr, args);
    },
  });

  pi.registerTool({
    name: "levi_task",
    label: "Levi Task",
    description:
      "Mutate a Levi task. Close only after the fixing commit exists; close is anchored at HEAD by default. Steal and no_anchor should only be used when explicitly justified.",
    promptSnippet: "Claim, drop, close, reopen, or comment on a Levi task",
    promptGuidelines: [
      "Use levi_task action=close only after committing the work that fixes the task.",
      "Use levi_task action=drop when abandoning claimed work; do not silently steal another developer's claim.",
    ],
    parameters: Type.Object({
      action: StringEnum(["start", "steal", "drop", "close", "reopen", "comment"] as const),
      id: Type.String({ description: "Task ID or unambiguous prefix" }),
      text: Type.Optional(Type.String({ description: "Required for comment" })),
      anchor: Type.Optional(Type.String({ description: "Explicit commit for close/reopen" })),
      no_anchor: Type.Optional(Type.Boolean({ description: "Apply close/reopen on every checkout" })),
      force: Type.Optional(Type.Boolean({ description: "Allow a redundant status transition" })),
      keep_claim: Type.Optional(Type.Boolean({ description: "Do not release the claim on close/reopen" })),
    }),
    async execute(_id, params, signal, _update, ctx) {
      if (params.action === "comment" && !params.text) throw new Error("text is required for comment");
      if (params.anchor && params.no_anchor) throw new Error("anchor and no_anchor conflict");
      const args = params.action === "comment"
        ? ["comment", params.id, params.text!]
        : [params.action, params.id];
      if (params.action === "close" || params.action === "reopen") {
        if (params.anchor) args.push("--anchor", params.anchor);
        if (params.no_anchor) args.push("--no-anchor");
        if (params.force) args.push("--force");
        if (params.keep_claim) args.push("--no-drop");
      }
      const result = await run(ctx.cwd, args, signal);
      changed(ctx);
      return toolResult(result.stdout, result.stderr, args);
    },
  });

  pi.registerTool({
    name: "levi_dep",
    label: "Levi Dependency",
    description: "Add or remove a local or cross-project Levi task dependency.",
    promptSnippet: "Manage blockers between Levi tasks",
    parameters: Type.Object({
      action: StringEnum(["add", "remove"] as const),
      blocked: Type.String(),
      blocker: Type.String({ description: "Local ID or project/lv-id[@ref]" }),
      via: Type.Optional(Type.String({ description: "How a cross-project dependency is consumed" })),
    }),
    async execute(_id, params, signal, _update, ctx) {
      const args = ["dep", params.action === "remove" ? "rm" : "add", params.blocked, "--on", params.blocker];
      if (params.via && params.action === "add") args.push("--via", params.via);
      const result = await run(ctx.cwd, args, signal);
      changed(ctx);
      return toolResult(result.stdout, result.stderr, args);
    },
  });

  pi.registerCommand("levi", {
    description: "Pick, inspect, or claim a Levi task",
    handler: async (_args, ctx) => {
      if (ctx.mode !== "tui") {
        ctx.ui.notify("/levi requires interactive mode", "error");
        return;
      }
      let tasks: Task[];
      try {
        tasks = (await json<TaskList>(ctx.cwd, ["ls", "--json"])).data.tasks;
      } catch (error) {
        ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
        return;
      }
      if (tasks.length === 0) {
        ctx.ui.notify("No open Levi tasks", "info");
        return;
      }
      const labels = tasks.map(taskLabel);
      const selected = await ctx.ui.select("Levi tasks", labels);
      if (!selected) return;
      const task = tasks[labels.indexOf(selected)];
      if (!task) return;
      const action = await ctx.ui.select(`${task.short}: ${task.title}`, [
        "Work on it (claim and start agent)",
        "Inspect it in the editor",
        "Claim only",
      ]);
      if (!action) return;
      if (action === "Inspect it in the editor") {
        ctx.ui.setEditorText(`Inspect ${task.short} with levi and summarize its current state.`);
        return;
      }
      try {
        await run(ctx.cwd, ["start", task.short]);
        changed(ctx);
        if (action === "Work on it (claim and start agent)") {
          pi.sendUserMessage(`Work on ${task.short}: ${task.title}`);
        } else {
          ctx.ui.notify(`Claimed ${task.short}`, "info");
        }
      } catch (error) {
        ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
      }
    },
  });

  pi.on("session_start", async (_event, ctx) => {
    await refreshStatus(ctx);
    if (!active || ctx.mode !== "tui") return;
    ctx.ui.addAutocompleteProvider((current) => ({
      triggerCharacters: ["-"],
      async getSuggestions(lines, line, col, options) {
        const beforeCursor = (lines[line] ?? "").slice(0, col);
        const match = beforeCursor.match(/(?:^|[\s(])((?:[\w.-]+\/)?lv-[\w-]*)$/);
        if (!match) return current.getSuggestions(lines, line, col, options);
        try {
          const tasks = await allTasks(ctx.cwd, options.signal);
          const query = match[1]!.toLowerCase();
          const items: AutocompleteItem[] = tasks
            .filter((task) => `${task.short} ${task.title}`.toLowerCase().includes(query))
            .slice(0, 20)
            .map((task) => ({ value: task.short, label: task.short, description: `${task.priority} ${task.status} · ${task.title}` }));
          return items.length > 0 ? { prefix: match[1]!, items } : current.getSuggestions(lines, line, col, options);
        } catch {
          return current.getSuggestions(lines, line, col, options);
        }
      },
      applyCompletion(lines, line, col, item, prefix) {
        return current.applyCompletion(lines, line, col, item, prefix);
      },
      shouldTriggerFileCompletion(lines, line, col) {
        return current.shouldTriggerFileCompletion?.(lines, line, col) ?? true;
      },
    }));
  });

  pi.on("session_shutdown", async (_event, ctx) => {
    ctx.ui.setStatus(STATUS_KEY, undefined);
  });
}
