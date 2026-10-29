# Demo GIF storyboard

[Documentation](README.md) · [Screenshots](screenshots.md) ·
[Get started](getting-started.md)

## Audience and message

This is for someone seeing Perseid for the first time. In under a minute, show
the manual-testing loop end to end: **capture traffic, edit and replay a
request, fuzz a parameter, decode an encoded response, and compare two
responses**. Keep the run approachable: use plain HTTP against a synthetic
loopback service, so there is no browser setup, certificate installation, or
real target involved.

The demo target deliberately returns base64 values from several endpoints. The
workflow uses them as realistic material for Repeater, Intruder, Decoder, and
Comparer instead of showing unrelated isolated screens.

## Sequence

| Approx. time | What appears | What a new user should take away |
| --- | --- | --- |
| 0–4 s | Open on **Proxy > HTTP history** with the proxy active; five local requests appear, including catalog, export, and product requests. | Requests passing through the proxy are recorded in one place. |
| 4–8 s | Select the product request with `id=2`, pause on its details, then choose **Send to Repeater**. | Any captured request can become a replayable request. |
| 8–13 s | Change `id=2` to `id=42`, click **Send**, and hold on the JSON response with its base64 receipt. | Repeater edits and replays exactly the request shown. |
| 13–16 s | Return to history, select the export request with `id=2`, and choose **Send to Intruder**. | The same capture can feed a payload-driven workflow. |
| 16–23 s | Click **Auto $$**, enter payloads `1`, `2`, and `42`, start the attack, and inspect the `id=42` result's base64 response. | Intruder marks a parameter, runs the payload set, and keeps each response reviewable. |
| 23–25 s | Return to history and select the export request with `id=1`. | Captures remain available while moving between tools. |
| 25–32 s | Choose **Send response to Decoder > Start new chain**, select the base64 body line, and click **Smart decode**. | Decoder can transform only the selected part and recognizes an encoding without a manual codec choice. |
| 32–43 s | Send both export responses to **Comparer**, select one item in each table, and click **Words**. Hold on the highlighted result window. | Comparer shows exactly what changed between two captured responses. |
| 43–47 s | Rest on the Comparer result. | The workflow ends on a useful, inspectable result rather than a control panel. |

## Recording direction

- Keep the whole GIF under 60 seconds; aim for roughly 45 seconds.
- Record one continuous, real app session. Avoid cuts, title cards, and unrelated
  features; the pointer and actual UI should carry the story.
- Move the pointer visibly to each row, menu item, editor, and button, then pause
  briefly so the action and result are easy to follow.
- Hold long enough to read the response or comparison, but do not linger on
  empty editors or loading states.
- Use only the loopback demo target and synthetic requests. The GIF must not
  capture personal data or traffic from the runner.

The executable version of this sequence lives in
[`scripts/record-demo.sh`](../scripts/record-demo.sh). The recording workflow
checks the exact set of target requests, resizes and quantizes the output to
1280×820, optimizes it with `gifsicle`, and enforces the duration and size
limits.
