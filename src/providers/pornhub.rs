use anyhow::{Context, Result};
use std::sync::OnceLock;
use std::time::Duration;

const PORNHUB_SEARCH_URL: &str = "https://www.pornhub.com/video/search";
const MAX_SEARCH_RESULTS: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PornhubVideoInfo {
    pub vkey: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<String>,
}

impl PornhubVideoInfo {
    #[allow(dead_code)]
    pub fn canonical_url(&self) -> String {
        format!("https://www.pornhub.com/view_video.php?viewkey={}", self.vkey)
    }

    pub fn button_label(&self) -> String {
        let clean_title = decode_html_entities(&self.title);
        let duration_suffix = match &self.duration {
            Some(d) if !d.trim().is_empty() => format!(" [{}]", d.trim()),
            _ => String::new(),
        };

        let max_title_chars = 52usize;
        let truncated_title = if clean_title.chars().count() > max_title_chars {
            let s: String = clean_title.chars().take(max_title_chars - 1).collect();
            format!("{s}…")
        } else {
            clean_title
        };

        format!("🔞 {truncated_title}{duration_suffix}")
    }
}

/// Checks whether the URL is a Pornhub or Thumbzilla video link.
pub fn is_pornhub_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("pornhub.com")
        || lower.contains("pornhub.org")
        || lower.contains("pornhub.net")
        || lower.contains("pornhubpremium.com")
        || lower.contains("thumbzilla.com")
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
}

/// Parses video search results from Pornhub search HTML.
pub fn parse_search_html(html: &str) -> Vec<PornhubVideoInfo> {
    let mut results = Vec::new();

    // Look for video items within #videoSearchResult or whole page
    let search_block = if let Some(start) = html.find("id=\"videoSearchResult\"") {
        if let Some(end) = html[start..].find("</ul>") {
            &html[start..start + end]
        } else {
            &html[start..]
        }
    } else {
        html
    };

    let mut pos = 0;
    while let Some(li_start) = search_block[pos..].find("data-video-vkey=\"") {
        let abs_start = pos + li_start + "data-video-vkey=\"".len();
        let Some(quote_end) = search_block[abs_start..].find('"') else {
            break;
        };
        let vkey = &search_block[abs_start..abs_start + quote_end];

        let li_end = search_block[abs_start..]
            .find("</li>")
            .map(|i| abs_start + i)
            .unwrap_or(search_block.len());

        let li_content = &search_block[abs_start..li_end];

        // Extract title
        let mut title = None;
        if let Some(title_start) = li_content.find("title=\"") {
            let t_start = title_start + "title=\"".len();
            if let Some(t_end) = li_content[t_start..].find('"') {
                let candidate = &li_content[t_start..t_start + t_end];
                if !candidate.is_empty() {
                    title = Some(decode_html_entities(candidate));
                }
            }
        }

        // Extract duration: <var ... class="...duration..." ...>10:24</var>
        let mut duration = None;
        if let Some(_dur_idx) = li_content.find("class=\"") {
            let mut search_from = 0;
            while let Some(var_idx) = li_content[search_from..].find("<var") {
                let var_start = search_from + var_idx;
                let var_tag_end = match li_content[var_start..].find('>') {
                    Some(i) => var_start + i,
                    None => break,
                };
                let var_tag = &li_content[var_start..var_tag_end];
                if var_tag.contains("duration") {
                    if let Some(close_var) = li_content[var_tag_end..].find("</var>") {
                        let text = li_content[var_tag_end + 1..var_tag_end + close_var].trim();
                        if !text.is_empty() {
                            duration = Some(text.to_string());
                            break;
                        }
                    }
                }
                search_from = var_tag_end + 1;
            }
        }

        // Extract uploader from usernameWrap
        let mut uploader = None;
        if let Some(u_idx) = li_content.find("usernameWrap") {
            let u_block = &li_content[u_idx..];
            if let Some(a_start) = u_block.find("<a") {
                if let Some(text_start) = u_block[a_start..].find('>') {
                    let full_start = a_start + text_start + 1;
                    if let Some(a_end) = u_block[full_start..].find("</a>") {
                        let name = u_block[full_start..full_start + a_end].trim();
                        if !name.is_empty() {
                            uploader = Some(name.to_string());
                        }
                    }
                }
            }
        }

        if let Some(t) = title {
            if !results.iter().any(|r: &PornhubVideoInfo| r.vkey == vkey) {
                results.push(PornhubVideoInfo {
                    vkey: vkey.to_string(),
                    title: t,
                    uploader,
                    duration,
                });
                if results.len() >= MAX_SEARCH_RESULTS {
                    break;
                }
            }
        }

        pos = li_end;
    }

    results
}

fn ph_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap_or_default()
    })
}

/// Searches Pornhub for videos matching the query.
pub async fn search_pornhub_videos(query: &str) -> Result<Vec<PornhubVideoInfo>> {
    let client = ph_http_client();

    let resp = client
        .get(PORNHUB_SEARCH_URL)
        .query(&[("search", query)])
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        )
        .header("Cookie", "platform=pc; accessAgeDisclaimerPH=1; accessPH=1")
        .header("Accept-Language", "en-US,en;q=0.9,ru;q=0.8")
        .send()
        .await
        .context("Failed to send search request to Pornhub")?
        .error_for_status()
        .context("Pornhub search returned non-success HTTP status")?;

    let html = resp
        .text()
        .await
        .context("Failed to read Pornhub search response HTML")?;

    let results = parse_search_html(&html);
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_pornhub_url() {
        assert!(is_pornhub_url("https://www.pornhub.com/view_video.php?viewkey=68efab7892fc4"));
        assert!(is_pornhub_url("https://rt.pornhub.com/view_video.php?viewkey=68efab7892fc4"));
        assert!(is_pornhub_url("http://pornhub.org/video/show?viewkey=123"));
        assert!(is_pornhub_url("https://www.thumbzilla.com/video/ph56c6114abd99a/test"));
        assert!(!is_pornhub_url("https://www.youtube.com/watch?v=abc"));
        assert!(!is_pornhub_url("https://music.yandex.ru/track/123"));
    }

    #[test]
    fn test_parse_search_html() {
        let sample_html = r#"
            <ul id="videoSearchResult">
                <li data-video-vkey="68efab7892fc4">
                    <a href="/view_video.php?viewkey=68efab7892fc4" title="Sample &amp; Test Video &#039;One&#039;">
                        <var class="duration">10:24</var>
                    </a>
                    <div class="usernameWrap"><a href="/user/alice">Alice</a></div>
                </li>
                <li data-video-vkey="6a72bbca01d86">
                    <a href="/view_video.php?viewkey=6a72bbca01d86" title="Second Video">
                        <var class="bgShadeEffect duration tooltipTrig">05:30</var>
                    </a>
                </li>
            </ul>
        "#;

        let results = parse_search_html(sample_html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].vkey, "68efab7892fc4");
        assert_eq!(results[0].title, "Sample & Test Video 'One'");
        assert_eq!(results[0].duration.as_deref(), Some("10:24"));
        assert_eq!(results[0].uploader.as_deref(), Some("Alice"));
        assert_eq!(results[0].button_label(), "🔞 Sample & Test Video 'One' [10:24]");

        assert_eq!(results[1].vkey, "6a72bbca01d86");
        assert_eq!(results[1].duration.as_deref(), Some("05:30"));
    }
}
