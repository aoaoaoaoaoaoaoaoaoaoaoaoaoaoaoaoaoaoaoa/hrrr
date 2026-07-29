use dwemer_poolrooms::chrome;

#[derive(Default)]
pub struct FoldCage {
    active: usize,
    prior: Vec<egui::Id>,
    next: Vec<egui::Id>,
}

impl FoldCage {
    pub fn take_keys(&mut self, ctx: &egui::Context) {
        if self.prior.is_empty()
            || !ctx.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab))
        {
            return;
        }
        self.active = (self.active + 1) % self.prior.len();
        let target = self.prior[self.active];
        ctx.memory_mut(|memory| {
            memory.move_focus(egui::FocusDirection::None);
            memory.request_focus(target);
        });
        ctx.request_repaint();
    }

    pub fn begin_pass(&mut self) {
        self.next.clear();
    }

    pub fn record(&mut self, witness: FocusWitness) {
        if witness.engaged {
            self.active = self.next.len();
        }
        self.next.push(witness.header);
    }

    pub fn end_pass(&mut self) {
        std::mem::swap(&mut self.prior, &mut self.next);
        self.active = self.active.min(self.prior.len().saturating_sub(1));
    }
}

#[derive(Clone, Copy)]
pub struct FocusWitness {
    header: egui::Id,
    engaged: bool,
}

pub fn section(
    ui: &mut egui::Ui,
    id: &'static str,
    title: &'static str,
    default_open: bool,
    add: impl FnOnce(&mut egui::Ui),
) -> (Option<chrome::FoldWake>, FocusWitness) {
    let header = ui.make_persistent_id(id).with("header");
    let top = ui.cursor().top();
    let wake = chrome::section(ui, id, title, default_open, add);
    let bottom = ui.cursor().top();
    let sentinel = ui.interact(
        egui::Rect::from_min_size(egui::pos2(ui.min_rect().left(), bottom), egui::Vec2::ZERO),
        header.with("focus-loop"),
        egui::Sense::focusable_noninteractive(),
    );
    let looped = sentinel.has_focus();
    if looped {
        ui.memory_mut(|memory| memory.request_focus(header));
        ui.ctx().request_repaint();
    }
    let rect = egui::Rect::from_min_max(
        egui::pos2(ui.min_rect().left(), top),
        egui::pos2(ui.max_rect().right(), bottom),
    );
    let pointer_engaged = ui.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pointer| rect.contains(pointer))
    });
    let header_focused = ui.memory(|memory| memory.has_focus(header));
    (
        wake,
        FocusWitness {
            header,
            engaged: pointer_engaged || header_focused || looped,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct Ids {
        headers: [egui::Id; 2],
        first_options: [egui::Id; 2],
        map: egui::Id,
    }

    fn key(modifiers: egui::Modifiers) -> egui::RawInput {
        egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: Some(egui::Key::Tab),
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..egui::RawInput::default()
        }
    }

    fn pass(ctx: &egui::Context, cage: &mut FoldCage, input: egui::RawInput) -> Ids {
        let mut ids = Ids {
            headers: [egui::Id::NULL; 2],
            first_options: [egui::Id::NULL; 2],
            map: egui::Id::NULL,
        };
        let _output = ctx.run_ui(input, |ui| {
            cage.take_keys(ui.ctx());
            cage.begin_pass();
            let mut first_options = [egui::Id::NULL; 2];
            let (_, first) = section(ui, "first", "first", true, |ui| {
                first_options[0] = ui.button("one").id;
                let _second = ui.button("two");
            });
            cage.record(first);
            let (_, second) = section(ui, "second", "second", true, |ui| {
                first_options[1] = ui.button("three").id;
            });
            cage.record(second);
            let map = ui.button("map control").id;
            ids = Ids {
                headers: [first.header, second.header],
                first_options,
                map,
            };
            cage.end_pass();
        });
        ids
    }

    #[test]
    fn tab_is_caged_and_shift_tab_crosses_fold_groups() {
        let ctx = egui::Context::default();
        let mut cage = FoldCage::default();
        let ids = pass(&ctx, &mut cage, egui::RawInput::default());

        let _ids = pass(&ctx, &mut cage, key(egui::Modifiers::NONE));
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(ids.headers[0]));
        let _ids = pass(&ctx, &mut cage, key(egui::Modifiers::NONE));
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(ids.first_options[0])
        );
        let _ids = pass(&ctx, &mut cage, key(egui::Modifiers::NONE));
        let _ids = pass(&ctx, &mut cage, key(egui::Modifiers::NONE));
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(ids.headers[0]));

        let _ids = pass(&ctx, &mut cage, key(egui::Modifiers::SHIFT));
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(ids.headers[1]));
        let _ids = pass(&ctx, &mut cage, key(egui::Modifiers::NONE));
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(ids.first_options[1])
        );
        let _ids = pass(&ctx, &mut cage, key(egui::Modifiers::NONE));
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(ids.headers[1]));
        assert_ne!(ctx.memory(|memory| memory.focused()), Some(ids.map));
    }
}
