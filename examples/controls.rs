use gpui::Role;
use gpui::component::input::{Input, InputState};
use gpui::test::TestSupportExt;
use gpui::{
    App, AppContext, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

struct Controls {
    enabled: bool,
    name: Entity<InputState>,
}

impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = cx.entity();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(
                div()
                    .id("toggle")
                    .test_support()
                    .role(Role::Button)
                    .aria_label("Toggle")
                    .on_click(move |_, _, cx| {
                        enabled.update(cx, |state, cx| {
                            state.enabled = !state.enabled;
                            cx.notify();
                        });
                    })
                    .child("Toggle"),
            )
            .child(
                div()
                    .id("state")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(if self.enabled { "on" } else { "off" })
                    .child(if self.enabled { "on" } else { "off" }),
            )
            .child(
                Input::new(&self.name)
                    .id("name")
                    .aria_label("Name")
                    .w(px(240.)),
            )
    }
}

fn main() {
    gpui::application().run(|cx: &mut App| {
        gpui::init(cx);
        let (window, _) = gpui::open_window(gpui::WindowOptions::default(), cx, |window, cx| {
            let name = cx.new(|cx| InputState::new(window, cx));
            cx.new(|_| Controls {
                enabled: false,
                name,
            })
        })
        .expect("open controls window");
        gpui_goo::gpui_adapter::attach(window, cx).expect("attach automation server");
    });
}
