# apple-intelligence

A rush plugin that answers rush's small jobs (session titles, feed summaries, quick ask) with Apple Intelligence's on-device model: free, private and offline.

Requirements: an Apple silicon Mac on macOS 26 or later with Apple Intelligence turned on (System Settings > Apple Intelligence & Siri), and Xcode or its command line tools with the macOS 26 SDK to build.

Install: open a session in this repository and rush offers to install it (it runs `install.sh`, then asks you to approve it). By hand:

    .rush/plugins/apple-intelligence/install.sh
    rush plugin approve apple-intelligence

On Linux, or a Mac without the macOS 26 SDK, the build fails and nothing is installed; rush runs fine without it.

It answers two requests from rush: `models.list` (the one model, `on-device`, and whether it is available now) and `models.ask` (a fresh session per ask: `system` as instructions, `prompt` as the question). The model holds about 4096 tokens in all; input that doesn't fit fails with an error saying it is "too small".
