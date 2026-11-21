//! AnyTask web access in the background: read pages as text (tables kept as rows) and search the web.

pub mod decide;
pub mod html;
pub mod net;
pub mod page;
pub mod prices;
pub mod search;
pub mod weather;

pub use decide::Decider;
pub use net::Net;
pub use page::{Page, http_client, read_web_page};
pub use prices::{ShopPrice, shop_prices};
pub use search::{Depth, GOOGLE_SEARCH_MODELS, GoogleSearch, SearchAnswer, SearchProvider, SearchResult, google_route_retry, google_search, pause_google_after, web_search};
pub use weather::{Weather, weather};

pub use reqwest::Client as HttpClient;
