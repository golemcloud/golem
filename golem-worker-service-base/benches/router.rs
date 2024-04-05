use criterion::{black_box, criterion_group, criterion_main, Criterion};
use golem_worker_service_base::http::tree::{make_path, Pattern, RadixNode};

criterion_group!(benches, radix_tree_all_matches);
criterion_main!(benches);

fn radix_tree_all_matches(c: &mut Criterion) {
    let num_routes = &[10, 20, 50, 100];

    let mut group = c.benchmark_group("matches");
    for (i, &len) in num_routes.into_iter().enumerate() {
        group.bench_function(format!("{i}/len={len}"), |b| {
            let (original, routes) = generate_routes(len);
            let radix_tree = build_radix_tree(&routes);

            let match_routes: Vec<_> = (0..1000).map(|_| generate_match_route(&original)).collect();

            b.iter_with_setup(
                || {
                    let route = fastrand::choice(&match_routes).unwrap();
                    let refs = route.iter().map(|s| s.as_str()).collect::<Vec<_>>();
                    black_box(refs)
                },
                |refs| {
                    let _ = radix_tree.matches(refs.as_slice());
                },
            );
        });
    }
    group.finish();
}

fn generate_routes(n: usize) -> (Vec<&'static str>, Vec<Vec<Pattern>>) {
    let mut original_routes = Vec::with_capacity(n);
    let mut used_routes = std::collections::HashSet::with_capacity(n);

    while original_routes.len() < n {
        let route = *fastrand::choice(ROUTES).unwrap();
        if !used_routes.contains(route) {
            original_routes.push(route);
            used_routes.insert(route);
        }
    }

    let patterns = original_routes.iter().map(|s| make_path(*s)).collect();
    (original_routes, patterns)
}

fn build_radix_tree(routes: &[Vec<Pattern>]) -> RadixNode<usize> {
    let mut radix_tree = RadixNode::default();
    for (index, route) in routes.iter().enumerate() {
        radix_tree.insert_path(route, index).unwrap();
    }
    radix_tree
}

fn generate_match_route(routes: &[&str]) -> Vec<String> {
    let route = fastrand::choice(routes).unwrap();
    route
        .trim_matches('/')
        .split('/')
        .map(|segment| {
            if segment.starts_with(':') {
                match segment {
                    _ => fastrand::u32(1..1000).to_string(),
                }
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
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
    "/posts/:post_id",
    "/posts/:post_id/likes",
    "/posts/:post_id/comments",
    "/posts/:post_id/comments/:comment_id",
    "/posts/:post_id/comments/:comment_id/replies",
    "/posts/:post_id/comments/:comment_id/replies/:reply_id",
    "/profiles/:id",
    "/profiles/:id/posts",
    "/profiles/:id/followers",
    "/profiles/:id/following",
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
    "/tags/:tag_id",
    "/tags/:tag_id/posts",
    "/tags/:tag_id/followers",
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
    "/communities",
    "/communities/:community_id",
    "/communities/:community_id/members",
    "/communities/:community_id/posts",
    "/communities/:community_id/rules",
    "/communities/:community_id/moderators",
    "/communities/:community_id/analytics",
    "/communities/:community_id/settings",
    "/moderation/reports",
    "/moderation/reports/:report_id",
    "/moderation/banned-users",
    "/moderation/banned-posts",
    "/moderation/banned-comments",
    "/moderation/filters",
    "/moderation/filters/:filter_id",
    "/moderation/keywords",
    "/moderation/keywords/:keyword_id",
    "/support/tickets",
    "/support/tickets/:ticket_id",
    "/support/faq",
    "/support/contact",
    "/jobs",
    "/jobs/:job_id",
    "/jobs/apply",
    "/jobs/apply/:job_id",
    "/press",
    "/press/releases",
    "/press/releases/:release_id",
    "/legal/terms",
    "/legal/privacy",
    "/legal/guidelines",
    "/legal/licenses",
    "/partners",
    "/partners/apply",
    "/partners/program",
    "/developers",
    "/developers/docs",
    "/developers/api",
    "/developers/api/:version_id",
];
