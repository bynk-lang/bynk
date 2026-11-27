---
level: patch
changelog: "`bynk deploy --move-agent <context>` plans a move of a context that hosts agents to another Worker script with its state: with `--dry-run` the plan lists each move set, the Workers each phase pushes and the callers rebound between them, and warns of the window (`bynk.deploy.agent_move_window`); the JSON plan carries them as a `moves` array. Shapes v1 cannot move are refused with a `reason` (`chained`, `partial_selection`, `order`, `routes`, `tombstones`, `renamed_in_place`, `both`), a name that matches no move is `bynk.deploy.move_agent_unused`, and a move outside `--dry-run` is still refused (`apply_not_yet`); the refusal of an unnamed move now names `--move-agent` as the flag that lets it through (#3565)"
---
