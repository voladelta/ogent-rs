# Steer Mode

Steer mode (`ogent --steer`) controls a Director session.

## Commands

| Command | Behavior |
| --- | --- |
| `/cancel` | Cancel the in-flight request |
| `/new` | Start a fresh child session |
| `/compact` | Ask for a handoff brief and compact into a child session |
| `/compact <focus>` | Compact with explicit focus |
| `/q` | Exit TUI |

`/complete` is deprecated. Finish by writing terminal `state.status` (`done`, `blocked`, `failed`, `partial`) and sending final assistant output.

## Notes

- Director tool restrictions still apply in steer mode.
- Worker delegation is still available via `dispatch_workers`.
- Steer cancellation preserves partial streamed output.
