// SPDX-License-Identifier: MPL-2.0

//! ABI 3 lifecycle macros and a single, serial foreground VM session.
use rune::ast::{self, Spanned};
use rune::compile;
use rune::macros::{quote, MacroContext, TokenStream};
use rune::parse::Parser;
use rune::{Any, ContextError, Module};

pub const COMPONENT_FACTORY: &str = "__note4c_component_v3_create";
pub const COMPONENT_DISPATCH: &str = "__note4c_component_v3_dispatch";
pub const UP: i64 = 0;
pub const DOWN: i64 = 1;
pub const BOOT: i64 = 2;
pub const CLICK: i64 = 0;
pub const LONG_PRESS: i64 = 1;

#[derive(Any, Clone, Copy, Debug, rune::alloc::clone::TryClone)]
#[rune(item = ::widget)]
pub struct InputEvent {
    #[rune(get)]
    pub keycode: i64,
    #[rune(get)]
    pub kind: i64,
    #[rune(get)]
    pub held_ms: u64,
}
impl From<panelet_core::input::InputEvent> for InputEvent {
    fn from(value: panelet_core::input::InputEvent) -> Self {
        Self {
            keycode: match value.button {
                panelet_core::Button::Up => UP,
                panelet_core::Button::Down => DOWN,
                panelet_core::Button::Confirm => BOOT,
            },
            kind: match value.kind {
                panelet_core::input::InputKind::Click => CLICK,
                panelet_core::input::InputKind::LongPress => LONG_PRESS,
            },
            held_ms: value.held_ms,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum ComponentEvent {
    Mount,
    Timer,
    Input(InputEvent),
    BeforeDestroy,
}

pub(crate) fn module() -> Result<Module, ContextError> {
    let mut module = Module::with_item(["widget"])?;
    module.ty::<InputEvent>()?;
    module.attribute_macro(["component"], component)?;
    module.function(["__unit"], require_unit).build()?;
    Ok(module)
}
pub(crate) fn input_module() -> Result<Module, ContextError> {
    let mut module = Module::with_item(["os", "input"])?;
    for (name, value) in [
        ("UP", UP),
        ("DOWN", DOWN),
        ("BOOT", BOOT),
        ("CLICK", CLICK),
        ("LONG_PRESS", LONG_PRESS),
    ] {
        module.constant([name], value).build()?;
    }
    Ok(module)
}
fn require_unit(value: rune::Value) -> crate::HostResult<()> {
    rune::from_value::<()>(value)
        .map_err(|_| crate::HostError::invalid_argument("lifecycle hook must return unit"))
}
fn component(
    cx: &mut MacroContext<'_, '_, '_>,
    input: &TokenStream,
    item: &TokenStream,
) -> compile::Result<TokenStream> {
    Parser::from_token_stream(input, cx.input_span()).eof()?;
    let mut implementation =
        Parser::from_token_stream(item, cx.macro_span()).parse_all::<ast::ItemImpl>()?;
    let mut constructor = false;
    let mut hooks: [Option<ast::Ident>; 3] = [None, None, None];
    let mut inputs: [Option<ast::Ident>; 6] = [None, None, None, None, None, None];
    for function in &mut implementation.functions {
        let name = cx.resolve(function.name)?.to_string();
        if name == "new" {
            if constructor
                || !function.args.is_empty()
                || function.async_token.is_some()
                || function.const_token.is_some()
            {
                return Err(compile::Error::msg(
                    function.span(),
                    "new must be synchronous and take no arguments",
                ));
            }
            constructor = true;
        }
        let mut selected = false;
        let mut retained = rune::alloc::Vec::new();
        for attribute in std::mem::take(&mut function.attributes) {
            let path = cx.stringify(&attribute.path)?.to_string().replace(' ', "");
            if !path.starts_with("widget::") {
                retained.try_push(attribute)?;
                continue;
            }
            if selected || name == "new" {
                return Err(compile::Error::msg(
                    attribute.span(),
                    "one hook attribute per method",
                ));
            }
            selected = true;
            let is_input = path == "widget::input";
            if function.async_token.is_none()
                || function.const_token.is_some()
                || function.args.len() != if is_input { 2 } else { 1 }
                || !matches!(
                    function.args.iter().next().map(|(a, _)| a),
                    Some(ast::FnArg::SelfValue(_))
                )
            {
                return Err(compile::Error::msg(
                    function.span(),
                    "async hook requires (self); input hook requires (self, event)",
                ));
            }
            let mut p = Parser::from_token_stream(&attribute.input, attribute.span());
            if is_input {
                p.parse::<rune::T!['(']>()?;
                let mut count = 0;
                loop {
                    let kind = p.parse::<ast::Ident>()?;
                    let kind = match cx.resolve(kind)? {
                        "click" => 0,
                        "long_press" => 1,
                        _ => {
                            return Err(compile::Error::msg(
                                attribute.span(),
                                "expected click or long_press",
                            ))
                        }
                    };
                    p.parse::<rune::T!['(']>()?;
                    loop {
                        let key = p.parse::<ast::Ident>()?;
                        let key = match cx.resolve(key)? {
                            "up" => 0,
                            "down" => 1,
                            "boot" => 2,
                            _ => {
                                return Err(compile::Error::msg(
                                    attribute.span(),
                                    "expected up, down or boot",
                                ))
                            }
                        };
                        let index = key * 2 + kind;
                        if inputs[index].is_some() {
                            return Err(compile::Error::msg(
                                attribute.span(),
                                "input event already bound",
                            ));
                        }
                        inputs[index] = Some(function.name);
                        count += 1;
                        if p.parse::<Option<rune::T![,]>>()?.is_none() {
                            break;
                        }
                    }
                    p.parse::<rune::T![')']>()?;
                    if p.parse::<Option<rune::T![,]>>()?.is_none() {
                        break;
                    }
                }
                p.parse::<rune::T![')']>()?;
                p.eof()?;
                if count == 0 {
                    return Err(compile::Error::msg(attribute.span(), "empty input binding"));
                }
            } else {
                p.eof()?;
                let index = match path.as_str() {
                    "widget::mounted" => 0,
                    "widget::timer" => 1,
                    "widget::before_destroy" => 2,
                    _ => {
                        return Err(compile::Error::msg(
                            attribute.span(),
                            "unknown lifecycle hook; render is an ordinary method",
                        ))
                    }
                };
                if hooks[index].is_some() {
                    return Err(compile::Error::msg(
                        attribute.span(),
                        "duplicate lifecycle hook",
                    ));
                }
                hooks[index] = Some(function.name);
            }
        }
        function.attributes = retained;
    }
    if !constructor {
        return Err(compile::Error::msg(
            implementation.span(),
            "component requires new()",
        ));
    }
    let state_type = &implementation.path;
    let state_type = quote!(#state_type).into_token_stream(cx)?;
    let mut lifecycle = Vec::new();
    for hook in hooks {
        lifecycle.push(if let Some(name) = hook {
            quote!(widget::__unit(state . #name().await?)? ;).into_token_stream(cx)?
        } else {
            TokenStream::new()
        });
    }
    let [mounted, timer, destroy]: [TokenStream; 3] = lifecycle.try_into().unwrap();
    let mut arms = TokenStream::new();
    use rune::macros::ToTokens;
    for (index, hook) in inputs.into_iter().enumerate() {
        if let Some(name) = hook {
            let key = cx.lit((index / 2) as i64)?;
            let kind = cx.lit((index % 2) as i64)?;
            quote!((#key , #kind) => { widget::__unit(state . #name(event).await?)? ; },)
                .into_token_stream(cx)?
                .to_tokens(cx, &mut arms)?;
        }
    }
    Ok(quote!(
        #implementation
        pub fn __note4c_component_v3_create() { #state_type::new() }
        pub async fn __note4c_component_v3_dispatch(state, kind, event) {
            match kind {
                0 => { #mounted },
                1 => { #timer },
                2 => { match (event.keycode,event.kind) { #arms _ => {} } },
                3 => { #destroy },
                _ => return os::runtime::fail("unknown lifecycle event"),
            }
            os::runtime::ok(())
        }
    )
    .into_token_stream(cx)?)
}

pub struct ComponentSession {
    vm: rune::Vm,
    bindings: crate::OsBindings,
    state: Option<rune::Value>,
    poisoned: bool,
    remaining: usize,
    memory_limit: usize,
    instructions: usize,
}
impl ComponentSession {
    pub fn new(
        vm: rune::Vm,
        bindings: crate::OsBindings,
        memory_limit: usize,
        instructions: usize,
    ) -> crate::HostResult<Self> {
        vm.lookup_function([COMPONENT_FACTORY]).map_err(vm_error)?;
        vm.lookup_function([COMPONENT_DISPATCH]).map_err(vm_error)?;
        Ok(Self {
            vm,
            bindings,
            state: None,
            poisoned: false,
            remaining: memory_limit,
            memory_limit,
            instructions,
        })
    }
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    pub async fn dispatch(
        &mut self,
        host: crate::Std,
        event: ComponentEvent,
    ) -> crate::HostResult<()> {
        if self.poisoned {
            return Err(crate::HostError::cancelled());
        }
        self.poisoned = true;
        let lease = BindingLease(self.bindings.clone());
        let vm = &mut self.vm;
        let state = &mut self.state;
        let bindings = &self.bindings;
        let remaining = &mut self.remaining;
        let destroy = matches!(event, ComponentEvent::BeforeDestroy);
        let limit = self.memory_limit;
        let result = rune::runtime::budget::with(
            self.instructions,
            rune::alloc::limit::with(*remaining, async {
                let result = async {
                    if state.is_none() {
                        if destroy {
                            return Ok(());
                        }
                        // Constructor runs without any active os invocation.
                        *state = Some(vm.call([COMPONENT_FACTORY], ()).map_err(vm_error)?);
                        bindings.activate(host.clone(), true)?;
                        checked(
                            vm.async_call(
                                [COMPONENT_DISPATCH],
                                (state.as_ref().unwrap().clone(), 0i64, rune::Value::from(())),
                            )
                            .await,
                        )?;
                        if !matches!(event, ComponentEvent::Input(_)) {
                            return Ok(());
                        }
                    } else {
                        bindings.activate(host, !destroy)?;
                    }
                    let (kind, event) = match event {
                        ComponentEvent::Mount => return Ok(()),
                        ComponentEvent::Timer => (1i64, rune::Value::from(())),
                        ComponentEvent::Input(event) => (
                            2,
                            rune::to_value(event)
                                .map_err(|_| crate::HostError::invalid_argument("input event"))?,
                        ),
                        ComponentEvent::BeforeDestroy => (3, rune::Value::from(())),
                    };
                    checked(
                        vm.async_call(
                            [COMPONENT_DISPATCH],
                            (state.as_ref().unwrap().clone(), kind, event),
                        )
                        .await,
                    )
                }
                .await;
                *remaining = rune::alloc::limit::get().min(limit);
                result
            }),
        )
        .await;
        drop(lease);
        self.poisoned = result.is_err() || destroy;
        result
    }
}
struct BindingLease(crate::OsBindings);
impl Drop for BindingLease {
    fn drop(&mut self) {
        self.0.revoke();
    }
}
fn vm_error(_: rune::runtime::VmError) -> crate::HostError {
    crate::HostError::invalid_argument("component execution failed or exceeded budget")
}
fn checked(value: Result<rune::Value, rune::runtime::VmError>) -> crate::HostResult<()> {
    let value = value.map_err(vm_error)?;
    let value = rune::from_value::<Result<rune::Value, crate::HostError>>(value)
        .map_err(|_| crate::HostError::invalid_argument("hook must return Result"))??;
    require_unit(value)
}
