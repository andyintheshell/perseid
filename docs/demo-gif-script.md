# Demo GIF storyboard

[Documentation](README.md) · [Screenshots](screenshots.md) ·
[Get started](getting-started.md)

## Audience and message

This is for someone seeing Perseid for the first time. In about half a minute,
show the basic manual-testing loop: **capture a request, inspect it, edit and
replay it, then inspect the response**. Keep the first run approachable: use
plain HTTP against a synthetic loopback service, so there is no browser setup,
certificate installation, or real target involved.

## Sequence

| Approx. time | What appears | What a new user should take away |
| --- | --- | --- |
| 0–3 s | Open on **Proxy > HTTP history** with the proxy active; let the app settle and place the pointer over the history table. | This is a desktop workbench with a live HTTP proxy. |
| 3–7 s | Three local `GET` requests appear in the history table, including a product request with `id=2`. | Requests passing through the proxy are recorded here. |
| 7–10 s | Select the product exchange and pause on its request and response details. | Select a history row to inspect both sides of an exchange. |
| 10–13 s | Open the row menu and choose **Send to Repeater**; show the prefilled editable request. | A captured request can become a replayable request. |
| 13–16 s | Change `id=2` to `id=42` in the raw request. | Repeater lets you edit a request before sending. |
| 16–25 s | Click **Send** and hold on the successful `200` JSON response. | The edited request is sent directly and its response is inspectable. |

## Recording direction

- Keep the whole GIF under 60 seconds; aim for roughly 25–30 seconds.
- Record one continuous, real app session. Avoid cuts, title cards, and unrelated
  features; the pointer and actual UI should carry the story.
- Move the pointer visibly to each row, menu item, editor, and **Send** button,
  then pause briefly so the action and result are easy to follow.
- End on the response, not on the editor or a loading state.
- Use only the loopback demo target and synthetic requests. The GIF must not
  capture personal data or traffic from the runner.

The executable version of this sequence lives in
[`scripts/record-demo.sh`](../scripts/record-demo.sh); the recording workflow
checks the target requests and enforces the duration and output resolution.
