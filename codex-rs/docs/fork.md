## Next goal: tree-structured prompt context overlay

Summaries and grouping should be reusable, so introduce a new crate dedicated to constructing the hierarchical prompt context tree. Codex continues recording `ResponseItem`s as before, but after each new user task it also calls an LLM to summarize per-task and per-turn details, yielding a structure like:

```
{
  "title": "task title",
  "type": "task",
  "summary": "task overview",
  "children": [
    {
      "title": "turn title",
      "type": "turn",
      "summary": "turn result",
      "raw": { /* original ResponseItem */ },
      "children": [ ... ]
    }
  ]
}
```

Requirements:

1) **Independent tree builder crate**
   - New crate builds the hierarchical structure from recorded history plus LLM-generated summaries.
   - Encapsulate schema and allow future experimentation without touching core logic.

2) **Core integration**
   - After each new user message, record history items as today, then invoke the tree builder to produce/update the hierarchical view using the summarization model.
   - Persist selection state per tree node; toggling a parent affects its descendants.

3) **TUI overlay updates**
   - `/context` overlay renders the tree with collapsible nodes (collapsed by default).
   - Navigation shows node summaries in the detail pane; allow expanding/collapsing recursively.
   - Toggling a node toggles all children in one action.

4) **Schema evolution**
   - Keep the overlay rendering logic decoupled from the tree builder so schema changes (different grouping, additional metadata) can be accommodated without large TUI rewrites.
