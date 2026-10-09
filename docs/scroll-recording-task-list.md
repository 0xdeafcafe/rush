# Scrolling recording investigation

- [x] Reproduce viewport movement caused by content-height recalculation: adding three rows below a scrolled answer moves the text, without any wheel event.
- [x] Preserve a preceding row reference for prose whose viewport has no selectable rows; the failing growth/shrink regression now passes.
- [x] Verify height changes across turns and build; install and reload the running view.

Plain answer rows have no selectable reference. Previously, a viewport filled with prose could have no scroll anchor, or anchor to a panel below it. It then moved when content below grew or shrank. The viewport now anchors to the nearest preceding referenced row, including above the visible window.

Validation: the growth/shrink regression failed before this fix (content height 304 → 307 moved the answer with no input) and passes afterward. A 20-turn regression keeps the expected visible rows while estimated heights are replaced with actual heights, including intervening redraws. Replay of the recorded transcript with unchanged content remained stable at six widths. Targeted scrolling, wheel, subagent-preview, prompt-pin, viewport, panel, minimap, and history-fold tests pass; the application builds. The installed binary matches the tested build, and `rush reload` reported one view reloaded. Live trackpad feel still needs confirmation. The full UI suite was not repeated: the earlier run hit sandbox socket restrictions and timed out in external model discovery.

The separate horizontal-event guard remains: sideways trackpad events no longer count as downward scrolling. Original three-row vertical sensitivity is restored.
