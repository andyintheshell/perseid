# Agent guidance

## Responses

Be concise in all responses. Avoid unnecessary narration, repetition, and explanations. Preserve complete code, exact commands, error messages, security considerations, and important implementation details. Do not sacrifice correctness or testing to reduce tokens.

## Tests and coverage

- The project-wide test-coverage target is 80%. Treat this as a goal to work
  toward, not a requirement for every pull request to raise total coverage to
  80% on its own.
- Pull requests should add or update automated tests for changed behavior
  whenever reasonable. Cover important success, error, and edge paths; explain
  in the pull request when meaningful behavior cannot reasonably be tested.
- Prefer tests that protect behavior over tests added only to increase the
  coverage percentage.
