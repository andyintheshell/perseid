# Project save and normal-exit guarantees

Save As and Save Copy submit the required workspace and file operation as one
nonblocking queue admission. Queue rejection fails the operation. The writer
must serialize and commit that workspace before attempting the file operation;
workspace failures cannot be hidden by a later successful metadata flush.
Retry submits a fresh snapshot, including when the UI's periodic snapshot cache
already considers that state queued.

Ordinary quit uses the existing temporary-project Save/Discard/Cancel flow.
For a saved project, it asynchronously waits for the required workspace commit
and durable metadata flush. If the workspace changes while waiting, another
acknowledged snapshot is required. Failure or ten seconds without acknowledgement
keeps the session open with Retry, Save Copy, Discard and Quit, and Cancel.
Saving a recovery copy keeps the session open; cancelling or failing that copy
does not discard the workspace. Discard and Quit explicitly bypasses this check.

The ten-second deadline is a UI acknowledgement deadline, not cancellation of an
in-progress database operation. Late replies cannot close the application after
timeout. A retry is ordered behind any earlier accepted operation. Force kill,
crash, and OS termination bypassing the cancellable close flow are not covered.
The final `on_exit` fallback only logs persistence failures; it is not the normal
save decision point.

Issue #136 still owns legacy blocking control admissions and writer teardown:
the existing shutdown join is unbounded, even after its reply timeout. The normal
quit acknowledgement deadline does **not** promise bounded process teardown.

## Manual quit/reopen check

1. Save a project, change scope/tool state, and immediately quit normally.
2. Reopen the file and verify the latest state, not just captured traffic.
3. With a stalled/failing writer, verify the window stays open and offers recovery.
4. Cancel a recovery copy; verify state is unchanged. Retry to a writable
   destination and reopen the resulting file.

Automated tests exercise incompatible workspace-table writes, saturated queue
admission, recovery after failure, late replies after timeout, failed switch
barriers, and the existing quit/save/reopen state machine. The interactive manual
check must be performed on a desktop; it is not claimed by the headless tests.
