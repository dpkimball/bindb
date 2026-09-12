## Summary
<!-- What changed and why -->

## Issue
Choose **one** for each linked issue (required). GitHub only auto-closes on **Closes** / **Fixes** / **Resolves**.

- [ ] **Closes** #___ — merging completes the issue’s acceptance criteria. Put `Closes #<N>` in this body (not only the checkbox).
- [ ] **Updates** #___ — progress only; the issue stays open. Put `Implements #<N>` in this body.

## Test plan
- [ ] **Deployed to local k8s `dev` before opening this PR** (`make b` / package equivalent after sourcing `env.sh`)
- [ ] Reported deploy outcome (success / error) in this PR
- [ ] Smoked the change in `dev` (service URL / NodePort — not unit tests only)
- [ ] Lint + tests passed locally on the feature branch
- [ ] Followed `.github/PR_CHECKLIST.md`

## Links
- Issue: #

Closes #   <!-- or Implements # if this PR only updates the issue -->

---
**Hard gate:** Do not open this PR until the **Deployed to local k8s `dev`** box is honestly checked (or marked N/A with reason for docs-only / no runtime impact).
**Issue gate:** Do not leave both Closes and Updates blank. Use `Closes` only when merge should close the issue.
