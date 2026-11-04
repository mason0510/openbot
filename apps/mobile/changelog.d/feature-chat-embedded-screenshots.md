### Added

- The attachment menu has Desktop screenshot: it asks the host to capture the screen and put the image in this chat with a short summary.
- A host `capture_screen` result shows a short caption with the source and size above the image in the thread.

### Changed

- Agents with Computer Use are told to call `capture_screen` when the user asks to see the screen, or after a GUI task they asked to verify, then summarize what is visible.
