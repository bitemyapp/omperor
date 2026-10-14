# Observatory scrolling exploration

## Preserved reference

Commit `399a531` on `codex/interface-redesign` preserves the approved continuous-list Observatory. It has independently scrollable Running and Finished lists inside an outer scrolling page, with pinned evidence below. The concern is strongest around the agent lists: the pointer's location determines whether a gesture moves a list or the entire page, and reaching an edge can disturb the surrounding context.

## Options

| Approach | Benefit | Trade-off | Assessment |
| --- | --- | --- | --- |
| Fixed agent area and separate evidence area | Removes the agent lists' scrolling ancestor while retaining independent Running and Finished positions. Headers, filters, and layout controls stay put. | A split must divide the available height; focused arrangements are useful on smaller screens. | Implemented for native review. |
| One agent table with Running and Finished sections | One scrolling surface for all agents; familiar sorting and keyboard navigation. | A large running group can push finished work far away. Sticky section headers or explicit jumps would be needed. | Worth a later comparison if independent lists still feel too busy. |
| One list with Running / Finished tabs | A single agent scrollbar and more width for names and metadata. | Hides one category at a time and weakens side-by-side monitoring. | Useful as a narrow-window variant, less compelling as the default. |
| Agent browser beside an evidence column | Makes scrolling regions clearly distinct and leaves a tall reading surface. | Competes for width with project navigation, two agent groups, and multiple pinned comparisons. | Promising on wide displays; poor as the only arrangement. |
| Hide scrollbars or suppress scroll chaining alone | Small visual change. | Retains the competing scroll containers and makes the boundary less discoverable. | Insufficient on its own. |

## First experiment: fixed sibling areas

The Observatory has a stationary native container. The agent browser and evidence area are siblings. Neither agent list is nested within another scrolling surface. Both lists keep continuous scrolling, explicit states, density controls, search, and independent task-based reading anchors. The evidence area retains native card text scrolling and, when multiple rows of cards need room, a separate evidence-grid scroller.

A local **Agents / Split / Pins** selector changes the space allocation immediately. Agents uses the full height for swarm browsing. Split keeps the lists above retained evidence. Pins gives the reading area to comparison. These arrangements preserve list positions, pinned records, text selections, and drafts; they do not create a new session or close a retained result. Selecting an agent from the focused Agents arrangement reveals its evidence in Split. In a narrow center column, it reveals Pins so the record remains readable.

The full Agents sidebar still owns browsing when it is open, so the center shows pins without duplicating the roster. Choosing Agents or Split in the Observatory closes that duplicate browser. When a different side panel leaves less than 720 points for the center, Split temporarily uses the Agents arrangement; widening the center restores the requested split. Pins remains available throughout.

## Review questions

- Does the stationary browser eliminate the feeling that the entire page might move while browsing agents?
- Is the split useful for quick inspection, or should a future native divider give the user continuous control over its height?
- Does the local arrangement selector make returning to a large swarm or a long comparison sufficiently immediate?
- With four pins, is scrolling the evidence grid acceptable, or would an evidence tab strip or horizontal shelf be better?
- Would a single sortable agent table add enough clarity to justify losing simultaneous independent Running and Finished scrolling?

The experiment keeps the approved list styling and evidence cards so the scroll structure can be judged separately from another visual redesign. It does not claim live distributed-agent scale testing.

## Validation of the first experiment

Native review covered dark and light appearances, 1440 × 900 and 1080 × 720 windows, scrolling past the end of an agent list, returning from Pins to the same list positions, switching sessions, compact density, immediate pin reveal, and two- and four-pin arrangements. The fixed browser does not move while the evidence grid scrolls. Single-row evidence grids omit their scrollbar when the cards fit; card text still scrolls independently. The full agent arrangement shows fourteen compact rows per group at 1080 × 720. The smaller Split arrangement intentionally trades some browsing capacity for an evidence preview. All 175 tests pass, including layout bounds and narrow-column fallback checks. Production compilation and the existing lint configuration also pass.

## Second experiment: space follows information

An empty category should not reserve half the Observatory. Filtering or searching now removes empty Running or Finished lanes. In Split, a single matching category uses a tall agent column beside the evidence; without pins it uses the full area. A search with no matches leaves a compact explanation and gives the remaining area to retained pins. With both categories populated, the familiar parallel lists remain above the comparison.

Pins give priority to the record itself. A compact header carries identity, status, and a one-line task notice. Long notices have a native disclosure control and a full tooltip. The previous large notice paragraph and separator no longer push the evidence down. Record identity, previous/next navigation, and Latest share one compact footer. Notices remain included when Discuss pins prepares an editable draft.

The evidence area adapts to its available width and height. Wide areas compare cards in two columns, with an odd final card spanning the last row. A sufficiently tall, narrower area stacks cards. A short narrow area offers named pin tabs and one readable native text surface, avoiding a stack of clipped cards and an extra outer scrollbar. Tab selection belongs to the session; choosing an agent brings its pin forward. Every pin retains its own record, selection, and reading position.

Each pin can expand into a reader without a modal. Compare all returns to the previous arrangement and restores the comparison's reading positions, including when the taller reader temporarily fits an entire record and clamps its native scroll position to zero. Switching sessions preserves the expanded reader, just as it preserves the comparison and conversation draft.

Split requires at least 760 points in the center. Below 900 points wide and 600 points high, two populated categories temporarily use the full Agents arrangement; the requested Split returns when space permits. Selecting an agent in that constrained arrangement reveals Pins immediately. A single filtered category can still share the compact window with a tabbed evidence reader. These are local responsive arrangements, not a global simple/expert setting.

### Design principle

Allocate space to the information the user is currently investigating, rather than to empty structural cells or repeated controls. A change in filter or window size may rearrange retained surfaces, but it must not discard the user's context. Prefer an immediate local expansion or named tab over a modal, hidden content, or a cramped approximation of a larger layout.

### Implementation and review

The layout is computed from bounded geometry and existing session state. The changes add no timers, I/O, background polling, or eager agent-row construction. Native list virtualization and cached Markdown rendering remain in place; hidden pin text is not rerendered. Geometry and reading-state tests cover empty categories, narrow layouts, minimum card size, odd card counts, and restoration after expansion.

Native release review covered Running and Attention filters, zero-match search, two/three/four-pin comparisons, previous/next/Latest records, expansion and return, session switching, and a 1080 × 720 window. At the default 1440 × 900 size, filtered Split shows fourteen comfortable running rows beside two pins. At 1080 × 720 it shows nine rows beside a tabbed reader, with its footer visible. The fixture remains illustrative; these checks do not claim live distributed-agent load testing.


## Current direction

The scroll experiments inform the [agent inspection redesign](AGENT_INSPECTION_DESIGN.md). Overview retains full-height Running and Finished lists. Inspect places a single selected agent beside the roster and preserves explicitly kept agents. Compare uses the full reading area for retained agents, with matching-record controls. The pin metaphor and pin-on-click behavior described above are historical checkpoints.
