use planner::*;

fn step(id: &str, text: &str, status: StepStatus) -> PlanStep {
    PlanStep {
        id: id.into(),
        text: text.into(),
        status,
    }
}
fn change(s: &PlanSnapshot, steps: Vec<PlanStep>, reason: Option<&str>) -> PlanUpdate {
    PlanUpdate {
        revision: s.revision,
        goal: "deliver report".into(),
        steps,
        explanation: reason.map(str::to_owned),
        new_plan: false,
    }
}
fn initial(mode: PlanMode) -> PlanSnapshot {
    let s = PlanSnapshot::new(mode);
    s.update(change(
        &s,
        vec![
            step("read", "read the source", StepStatus::Pending),
            step("write", "write report", StepStatus::Pending),
        ],
        None,
    ))
    .unwrap()
}
#[test]
fn complete_state_is_immutable_but_unfinished_steps_can_be_revised_with_a_reason() {
    let mut s = initial(PlanMode::Normal);
    let mut steps = s.steps.clone();
    steps[0].status = StepStatus::Completed;
    s = s.update(change(&s, steps, None)).unwrap();
    let mut revised = s.steps.clone();
    revised[1].text = "produce a shorter report".into();
    assert!(s.update(change(&s, revised.clone(), None)).is_err());
    let next = s
        .update(change(&s, revised, Some("user reduced the scope")))
        .unwrap();
    assert_eq!(next.steps[0], s.steps[0]);
    assert_eq!(next.explanation.as_deref(), Some("user reduced the scope"));
    for edit in 0..4 {
        let mut steps = s.steps.clone();
        match edit {
            0 => {
                steps.remove(0);
            }
            1 => steps[0].text = "different action".into(),
            2 => steps[0].status = StepStatus::Pending,
            _ => steps[0].id = "other".into(),
        }
        assert!(s
            .update(change(&s, steps, Some("not permission to rewrite facts")))
            .is_err());
    }
}
#[test]
fn pending_add_remove_reorder_and_regression_require_explanations() {
    let s = initial(PlanMode::Normal);
    let mut steps = s.steps.clone();
    steps.reverse();
    assert!(s.update(change(&s, steps.clone(), None)).is_err());
    assert!(s
        .update(change(&s, steps, Some("dependency changed")))
        .is_ok());
    let mut steps = s.steps.clone();
    steps.push(step("check", "check result", StepStatus::Pending));
    assert!(s.update(change(&s, steps.clone(), None)).is_err());
    assert!(s
        .update(change(&s, steps, Some("add acceptance check")))
        .is_ok());
    let mut steps = s.steps.clone();
    steps.pop();
    assert!(s.update(change(&s, steps.clone(), None)).is_err());
    assert!(s
        .update(change(&s, steps, Some("no longer requested")))
        .is_ok());
    let mut steps = s.steps.clone();
    steps[0].status = StepStatus::InProgress;
    let active = s.update(change(&s, steps, None)).unwrap();
    assert!(active
        .update(change(&active, s.steps.clone(), None))
        .is_err());
}
#[test]
fn normal_progress_is_not_an_approval_and_completion_does_not_start_anything() {
    let s = initial(PlanMode::Normal);
    let mut steps = s.steps.clone();
    steps[0].status = StepStatus::InProgress;
    let s = s.update(change(&s, steps, None)).unwrap();
    assert_eq!(s.mode, PlanMode::Normal);
    assert!(s.proposal.is_none());
    let done = s
        .steps
        .iter()
        .cloned()
        .map(|mut t| {
            t.status = StepStatus::Completed;
            t
        })
        .collect();
    let s = s.update(change(&s, done, None)).unwrap();
    assert!(s.is_complete());
    assert!(s.resume_execution(s.revision).is_err());
    let mut new = change(
        &s,
        vec![step("next", "next task", StepStatus::Pending)],
        Some("new user task"),
    );
    new.new_plan = true;
    new.goal = "next goal".into();
    assert_eq!(s.update(new).unwrap().goal, "next goal");
}
#[test]
fn explicit_mode_keeps_the_exact_proposal_and_only_host_can_resume_a_matching_revision() {
    let s = initial(PlanMode::PlanOnly);
    let mut steps = s.steps.clone();
    steps[0].status = StepStatus::InProgress;
    assert!(s.update(change(&s, steps, None)).is_err());
    assert!(s.submit(s.revision, "not headed markdown".into()).is_err());
    let proposal =
        "# Delivery plan\nRead sources first; do not publish without separate authorization.";
    let pending = s.submit(s.revision, proposal.into()).unwrap();
    assert_eq!(pending.proposal.as_deref(), Some(proposal));
    assert!(pending
        .update(change(&pending, pending.steps.clone(), None))
        .is_err());
    assert!(pending.resume_execution(s.revision).is_err());
    let refined = pending.enter_plan_mode(pending.revision).unwrap();
    assert!(refined.proposal.is_none());
    assert!(refined.resume_execution(pending.revision).is_err());
    let continued = pending.resume_execution(pending.revision).unwrap();
    assert_eq!(continued.mode, PlanMode::Normal);
    assert_eq!(continued.proposal.as_deref(), Some(proposal));
    assert!(!continued.awaits_host());
    assert!(pending.awaits_host());
    assert_eq!(continued.steps, pending.steps);
    let mut progressed = continued.steps.clone();
    progressed[0].status = StepStatus::Completed;
    let next = continued
        .update(change(&continued, progressed, None))
        .unwrap();
    assert_eq!(next.proposal.as_deref(), Some(proposal));
}
#[test]
fn state_validation_bounds_versions_identifiers_text_counts_and_total_serialized_size() {
    let s = initial(PlanMode::Normal);
    for case in 0..7 {
        let mut broken = s.clone();
        match case {
            0 => broken.version += 1,
            1 => broken.steps[1].id = broken.steps[0].id.clone(),
            2 => broken.steps[1].text = broken.steps[0].text.clone(),
            3 => broken.steps[0].text = " ".into(),
            4 => broken.steps[0].text = "中".repeat(400),
            5 => broken.steps[0].id = "../other".into(),
            _ => {
                for t in &mut broken.steps {
                    t.status = StepStatus::InProgress;
                }
            }
        }
        assert!(broken.validate().is_err(), "case {case}");
    }
    let mut huge = PlanSnapshot {
        goal: "huge".into(),
        steps: (0..33)
            .map(|i| step(&format!("s{i}"), &format!("task {i}"), StepStatus::Pending))
            .collect(),
        ..Default::default()
    };
    assert!(huge.validate().is_err());
    huge.steps.pop();
    for t in &mut huge.steps {
        t.text = format!("{}{}", t.id, "\\\"".repeat(300));
    }
    assert!(huge.validate().is_err());
    let mut overflow = s.clone();
    overflow.revision = u64::MAX;
    assert!(overflow.reset(u64::MAX).is_err());
    assert!(s
        .update(PlanUpdate {
            revision: 0,
            ..change(&s, s.steps.clone(), None)
        })
        .is_err());
}
#[test]
fn host_binding_is_atomic_and_respects_existing_metadata() {
    let s = initial(PlanMode::PlanOnly);
    let mut request = api::RunRequest::new("task");
    request.metadata.insert("tenant".into(), "trusted".into());
    bind_state(&mut request, &s).unwrap();
    assert_eq!(request.metadata["tenant"], "trusted");
    let before = request.metadata.clone();
    let mut invalid = s.clone();
    invalid.version = 99;
    assert!(bind_state(&mut request, &invalid).is_err());
    assert_eq!(request.metadata, before);
    request
        .metadata
        .insert("large".into(), "x".repeat(16 * 1024));
    let before = request.metadata.clone();
    assert!(bind_state(&mut request, &s).is_err());
    assert_eq!(request.metadata, before);
}
