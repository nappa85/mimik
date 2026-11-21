//! The shared header of every page of the authenticated app: the menu entries of the signed-in
//! account's role, built once per request from the account summary and CSRF token that
//! `principal` already loaded (no second session lookup).
use axum::response::IntoResponse;
use std::cell::RefCell;

/// The part of the app a page belongs to; its menu entry carries `aria-current="page"`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Section {
    Home,
    Campaigns,
    Sheets,
    Materials,
    Upload,
    Rulesets,
    Accounts,
    Assistant,
}

pub(crate) struct Item {
    pub label: &'static str,
    pub href: &'static str,
    pub current: bool,
}

pub(crate) struct Menu {
    pub label: &'static str,
    pub items: Vec<Item>,
}

/// What `principal` loaded for the request, kept for the page template.
#[derive(Clone)]
pub(super) struct Context {
    pub display_name: String,
    pub owner: bool,
    pub csrf: String,
}

tokio::task_local! { static CONTEXT: RefCell<Option<Context>>; }

pub(super) async fn scope<F: std::future::Future>(future: F) -> F::Output {
    CONTEXT.scope(RefCell::new(None), future).await
}

pub(super) fn forget() {
    let _ = CONTEXT.try_with(|slot| *slot.borrow_mut() = None);
}

pub(super) fn remember(context: Context) {
    let _ = CONTEXT.try_with(|slot| *slot.borrow_mut() = Some(context));
}

/// The header of one page: menus by role, the account menu, and the script flag.
pub(crate) struct Nav {
    /// `false` renders the logo-only header (no session).
    pub signed_in: bool,
    pub menus: Vec<Menu>,
    pub display_name: String,
    pub csrf: String,
    /// Strict (sheet) pages carry no script of the layout, so their script invariants hold.
    pub strict: bool,
    /// The page is the home page: the logo link carries `aria-current`.
    pub home: bool,
}

impl Nav {
    /// The header for the signed-in account of the current request.
    pub(crate) fn here(current: Section) -> Self {
        let scoped = CONTEXT.try_with(|slot| slot.borrow().clone());
        debug_assert!(
            scoped.is_ok(),
            "a page was rendered outside the request boundary (or on another thread)"
        );
        let context = scoped.ok().flatten();
        context.map_or_else(Self::logo_only, |context| Self::build(&context, current))
    }

    /// As [`Nav::here`], for a page of a sheet (the sheet itself, its history revisions and
    /// catalogues) or an owner material or upload page: the layout adds no script.
    pub(crate) fn here_strict(current: Section) -> Self {
        Self {
            strict: true,
            ..Self::here(current)
        }
    }

    pub(crate) fn logo_only() -> Self {
        Self {
            signed_in: false,
            menus: Vec::new(),
            display_name: String::new(),
            csrf: String::new(),
            strict: false,
            home: false,
        }
    }

    fn build(context: &Context, current: Section) -> Self {
        let item = |label, href, section| Item {
            label,
            href,
            current: section == current,
        };
        let mut menus = Vec::new();
        if context.owner {
            menus.push(Menu {
                label: "Materials",
                items: vec![
                    item("Sources and review", "/admin/materials", Section::Materials),
                    item("Upload PDF", "/admin/materials/uploads", Section::Upload),
                    item("Rulesets", "/admin/rulesets", Section::Rulesets),
                ],
            });
            menus.push(Menu {
                label: "Administration",
                items: vec![
                    item("Accounts", "/admin", Section::Accounts),
                    item("Assistant", "/admin/assistant", Section::Assistant),
                ],
            });
        } else {
            menus.push(Menu {
                label: "Play",
                items: vec![
                    item("Campaigns", "/campaigns", Section::Campaigns),
                    item("My sheets", "/private-sheets", Section::Sheets),
                ],
            });
        }
        Self {
            signed_in: true,
            menus,
            display_name: context.display_name.clone(),
            csrf: context.csrf.clone(),
            strict: false,
            home: current == Section::Home,
        }
    }
}

/// The small same-origin script that closes open menus (`assets/nav.js`).
pub(super) async fn script() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        include_str!("../../assets/nav.js"),
    )
}

/// Assertions on the rendered header, shared by the page tests of every renderer.
#[cfg(test)]
pub(crate) mod check {
    pub const OWNER_ENTRIES: [&str; 6] = [
        "/admin/materials",
        "/admin/materials/uploads",
        "/admin/rulesets",
        "/admin",
        "/admin/assistant",
        "/auth/login",
    ];
    pub const PLAYER_ENTRIES: [&str; 3] = ["/campaigns", "/private-sheets", "/auth/login"];

    /// Every `<a href>` of the menu with whether it carries `aria-current="page"`, and the
    /// summary texts.
    pub fn menu(html: &str) -> (Vec<(String, bool)>, Vec<String>) {
        let start = html.find("<nav class=\"menu\"").expect("a menu");
        let end = start + html[start..].find("</nav>").expect("a closed menu");
        let nav = &html[start..end];
        let links = nav
            .split("<a href=\"")
            .skip(1)
            .map(|chunk| {
                let (href, rest) = chunk.split_once('"').unwrap();
                let tag = rest.split_once('>').unwrap().0;
                (href.to_owned(), tag.contains("aria-current=\"page\""))
            })
            .collect();
        let summaries = nav
            .split("<summary>")
            .skip(1)
            .map(|chunk| {
                chunk.split_once("</summary>").unwrap().0.replace(
                    "<span class=\"visually-hidden\">Account: </span>",
                    "Account: ",
                )
            })
            .collect();
        (links, summaries)
    }

    /// The header of `html` is the role's menu with `current` marked, a Sign out form with a
    /// CSRF token, the nav script exactly when the page is not strict, and no inline code.
    pub fn assert_header(html: &str, owner: bool, current: Option<&str>, strict: bool, at: &str) {
        let (links, _) = menu(html);
        let expected: &[&str] = if owner {
            &OWNER_ENTRIES
        } else {
            &PLAYER_ENTRIES
        };
        let hrefs: Vec<&str> = links.iter().map(|(href, _)| href.as_str()).collect();
        assert_eq!(hrefs, expected, "{at}");
        let marked: Vec<&str> = links
            .iter()
            .filter(|(_, marked)| *marked)
            .map(|(href, _)| href.as_str())
            .collect();
        assert_eq!(marked, current.into_iter().collect::<Vec<_>>(), "{at}");
        assert!(html.contains("<a class=\"logo\" href=\"/\""), "{at}");
        let signout = html
            .split("action=\"/auth/logout\"><input type=\"hidden\" name=\"csrf\" value=\"")
            .nth(1)
            .unwrap_or_else(|| panic!("{at}: a Sign out form"));
        assert_eq!(
            signout.find('"'),
            Some(64),
            "{at}: sign out carries the CSRF token"
        );
        assert_eq!(
            html.contains("src=\"/assets/nav.js\""),
            !strict,
            "{at}: the nav script is left out of strict pages only"
        );
        assert!(!html.contains(" style="), "{at}");
        assert_eq!(
            html.matches("<script").count(),
            html.matches("<script src=").count()
                + html.matches("<script type=\"module\" src=").count(),
            "{at}: no inline script"
        );
        let other: &[&str] = if owner {
            &PLAYER_ENTRIES[..2]
        } else {
            &OWNER_ENTRIES[..5]
        };
        assert!(other.iter().all(|entry| !hrefs.contains(entry)), "{at}");
    }
}
