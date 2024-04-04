use divan::Bencher;
use golem_worker_service_base::http::router::{make_path, Pattern, RadixNode};

fn main() {
    divan::main();
}

const LENGTHS: &[usize] = &[10, 20, 50, 100];

const SAMPLE_COUNT: u32 = 1000;
const SAMPLE_SIZE: u32 = 100;

#[divan::bench(
    args = LENGTHS,
    sample_count = SAMPLE_COUNT,
    sample_size = SAMPLE_SIZE
)]
fn radix_tree_matches(bencher: Bencher, len: usize) {
    let (original, routes) = generate_routes(len);
    let radix_tree = build_radix_tree(&routes);

    bencher
        .with_inputs(|| generate_match_route(&original))
        .bench_values(|route| {
            let result = radix_tree.matches_str(&route);
            if result.is_none() {
                println!("No match found for route: {}", route);
            }
        });
}

fn generate_routes(n: usize) -> (Vec<&'static str>, Vec<Vec<Pattern>>) {
    let original_routes = (0..n)
        .map(|_| {
            let index = fastrand::usize(..ROUTES.len());
            ROUTES[index]
        })
        .collect::<Vec<_>>();

    let patterns = original_routes.iter().map(|s| make_path(*s)).collect();

    (original_routes, patterns)
}

fn build_radix_tree(routes: &[Vec<Pattern>]) -> RadixNode<usize> {
    let mut radix_tree = RadixNode::default();
    for (index, route) in routes.iter().enumerate() {
        let _ = radix_tree.insert_path(route, index);
    }
    radix_tree
}

fn generate_match_route(routes: &[&str]) -> String {
    let index = fastrand::usize(..routes.len());
    let route = routes[index];
    let result = route
        .trim_matches('/')
        .split('/')
        .map(|segment| {
            if segment.starts_with(':') {
                match segment {
                    s if s.ends_with("id") => fastrand::u32(1..1000).to_string(),
                    ":query" => generate_query(),
                    ":username" => generate_username(),
                    _ => "42".to_string(),
                }
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/");

    format!("/{}", result)
}

fn generate_query() -> String {
    let mut result = String::new();
    let length = fastrand::usize(1..10);
    for _ in 0..length {
        let word_length = fastrand::usize(3..8);
        for _ in 0..word_length {
            let ch = fastrand::u8(b'a'..=b'z') as char;
            result.push(ch);
        }

        result.push('+');
    }
    result.pop();
    result
}

fn generate_username() -> String {
    let mut result = String::new();
    let length = fastrand::usize(5..15);
    for _ in 0..length {
        let ch = fastrand::u8(b'a'..=b'z') as char;
        result.push(ch);
    }
    result
}

const ROUTES: &[&str] = &[
    "/",
    "/users",
    "/users/:id",
    "/users/:id/profile",
    "/users/:id/posts",
    "/users/:id/posts/:post_id",
    "/users/:id/posts/:post_id/comments",
    "/users/:id/posts/:post_id/comments/:comment_id",
    "/users/:id/followers",
    "/users/:id/following",
    "/posts",
    "/posts/trending",
    "/posts/latest",
    "/posts/search/:query",
    "/posts/:post_id",
    "/posts/:post_id/likes",
    "/posts/:post_id/comments",
    "/posts/:post_id/comments/:comment_id",
    "/posts/:post_id/comments/:comment_id/replies",
    "/posts/:post_id/comments/:comment_id/replies/:reply_id",
    "/profiles/:username",
    "/profiles/:username/posts",
    "/profiles/:username/followers",
    "/profiles/:username/following",
    "/api/v1/users",
    "/api/v1/users/:id",
    "/api/v1/users/:id/posts",
    "/api/v1/posts",
    "/api/v1/posts/:post_id",
    "/api/v1/posts/:post_id/comments",
    "/api/v2/users",
    "/api/v2/users/:id",
    "/api/v2/users/:id/timeline",
    "/api/v2/posts",
    "/api/v2/posts/trending",
    "/assets/*path",
    "/static/*filepath",
    "/admin",
    "/admin/users",
    "/admin/users/:id",
    "/admin/posts",
    "/admin/posts/:post_id",
    "/admin/comments",
    "/admin/comments/:comment_id",
    "/analytics",
    "/analytics/users",
    "/analytics/posts",
    "/analytics/comments",
    "/auth/login",
    "/auth/register",
    "/auth/forgot-password",
    "/auth/reset-password",
    "/settings",
    "/settings/profile",
    "/settings/account",
    "/settings/notifications",
    "/settings/privacy",
    "/messages",
    "/messages/:id",
    "/messages/:id/reply",
    "/notifications",
    "/notifications/:id",
    "/search",
    "/search/users",
    "/search/posts",
    "/search/comments",
    "/trending",
    "/trending/users",
    "/trending/posts",
    "/trending/tags",
    "/explore",
    "/explore/users",
    "/explore/posts",
    "/explore/tags",
    "/tags/:tag",
    "/tags/:tag/posts",
    "/tags/:tag/followers",
    "/feed",
    "/feed/latest",
    "/feed/trending",
    "/feed/suggested",
    "/activity",
    "/activity/likes",
    "/activity/comments",
    "/activity/follows",
    "/suggestions",
    "/suggestions/users",
    "/suggestions/posts",
    "/suggestions/tags",
    "/discover",
    "/discover/users",
    "/discover/posts",
    "/discover/tags",
    "/bookmarks",
    "/bookmarks/posts",
    "/bookmarks/comments",
    "/favorites",
    "/favorites/posts",
    "/favorites/comments",
    "/favorites/tags",
    "/dashboard",
    "/dashboard/overview",
    "/dashboard/analytics",
    "/dashboard/settings",
];
