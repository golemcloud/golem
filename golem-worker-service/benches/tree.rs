use criterion::{black_box, criterion_group, criterion_main, Criterion};
use golem_worker_service::gateway_execution::router::{tree::RadixNode, RouterPattern};

criterion_group!(benches, radix_tree_all_matches);
criterion_main!(benches);

const NUM_ROUTES: &[usize] = &[10, 20, 50, 100];
const HIT_RATES: &[u8] = &[25, 50, 75];

fn radix_tree_all_matches(c: &mut Criterion) {
    let mut group = c.benchmark_group("matches");
    for &hit_rate in HIT_RATES.iter() {
        for &len in NUM_ROUTES.iter() {
            group.bench_function(format!("len={len}/hit={hit_rate}"), |b| {
                let routes = generate_routes(len);
                let radix_tree = build_radix_tree(&routes);

                let (match_routes, miss_routes) = build_routes(&radix_tree);

                b.iter_with_setup(
                    || {
                        let hit = fastrand::u8(0..100) < hit_rate;
                        let route = if hit {
                            fastrand::choice(&match_routes).unwrap()
                        } else {
                            fastrand::choice(&miss_routes).unwrap()
                        };
                        let refs = route.iter().map(|s| s.as_str()).collect::<Vec<_>>();
                        black_box((hit, refs))
                    },
                    |(hit, refs)| {
                        let result = radix_tree.matches(refs.as_slice());
                        if hit {
                            assert!(result.is_some(), "{result:?} {refs:?}");
                        } else {
                            assert!(result.is_none(), "{result:?} {refs:?}");
                        }
                    },
                );
            });
        }
    }
    group.finish();
}

/// Will choose n unique routes from the list of all possible routes
fn generate_routes(n: usize) -> Vec<Vec<RouterPattern>> {
    let mut result = Vec::with_capacity(n);
    let mut used_routes = std::collections::HashSet::with_capacity(n);

    while result.len() < n {
        let route = *fastrand::choice(ROUTES).unwrap();
        if !used_routes.contains(route) {
            let pattern = RouterPattern::parse(route);
            result.push(pattern);
            used_routes.insert(route);
        }
    }

    result
}

/// Build a radix tree from a list of routes
fn build_radix_tree(routes: &[Vec<RouterPattern>]) -> RadixNode<(usize, String)> {
    let mut radix_tree = RadixNode::default();
    for (index, route) in routes.iter().enumerate() {
        let display_route = route
            .iter()
            .map(|segment| match segment {
                RouterPattern::Literal(literal) => literal.0.clone(),
                RouterPattern::Variable => "{var}".to_string(),
                RouterPattern::CatchAll => "*".to_string(),
            })
            .collect::<Vec<_>>()
            .join("/");

        let data = (index, display_route);
        radix_tree
            .insert_path(route, data)
            .expect("Conflict in routes");
    }
    radix_tree
}

// Generate a list of routes that will match and a list of routes that will not match
fn build_routes<T>(router: &RadixNode<T>) -> (Vec<Vec<String>>, Vec<Vec<String>>) {
    let all_patterns = ROUTES.iter().map(RouterPattern::parse).collect::<Vec<_>>();

    let (match_patterns, miss_patterns) =
        all_patterns.into_iter().partition::<Vec<_>, _>(|route| {
            let route_str = generate_match_route(route);
            let route_str = route_str.iter().map(|s| s.as_str()).collect::<Vec<_>>();
            router.matches(&route_str).is_some()
        });

    let match_routes = (0..500)
        .map(|_| generate_match_route(fastrand::choice(&match_patterns).unwrap()))
        .collect::<Vec<_>>();

    let miss_routes = (0..500)
        .map(|_| generate_match_route(fastrand::choice(&miss_patterns).unwrap()))
        .collect::<Vec<_>>();

    (match_routes, miss_routes)
}

fn generate_match_route(route: &[RouterPattern]) -> Vec<String> {
    route
        .iter()
        .flat_map(|segment| match segment {
            RouterPattern::Literal(literal) => vec![literal.0.clone()],
            RouterPattern::Variable => vec![fastrand::u32(1..1000).to_string()],
            RouterPattern::CatchAll => (1..10)
                .map(|_| fastrand::u32(1..1000).to_string())
                .collect::<Vec<_>>(),
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
    "/assets/path/:asset_id",
    "/static/filepath/:file_id",
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
