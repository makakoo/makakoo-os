import sys
import asyncio
import os
import urllib.parse
from playwright.async_api import async_playwright
from bs4 import BeautifulSoup
from markdownify import markdownify as md

SESSION_FILE = os.path.join(os.path.dirname(__file__), ".linkedin_session")

async def get_browser_context(p):
    if not os.path.exists(SESSION_FILE):
        print("❌ Error: .linkedin_session file not found.")
        sys.exit(1)

    with open(SESSION_FILE, 'r') as f:
        li_at_cookie = f.read().strip()

    browser = await p.chromium.launch(headless=True, args=["--disable-blink-features=AutomationControlled"])
    context = await browser.new_context(
        user_agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36"
    )
    
    await context.add_cookies([{
        "name": "li_at",
        "value": li_at_cookie,
        "domain": ".www.linkedin.com",
        "path": "/"
    }])
    return browser, context

async def scrape_job(context, url):
    page = await context.new_page()
    print(f"📡 Harvey Vision: Reading Job -> {url}")
    try:
        await page.goto(url, wait_until="domcontentloaded", timeout=20000)
        await page.wait_for_timeout(3000)
        html_content = await page.content()
        soup = BeautifulSoup(html_content, "html.parser")
        
        main_content = soup.find("div", {"class": "jobs-description__content"})
        if not main_content:
            main_content = soup.find("main", {"id": "main"})
        if not main_content:
            main_content = soup.body
            
        if main_content:
            for tag in main_content(['svg', 'img', 'nav', 'footer', 'button']):
                tag.decompose()
            clean_md = md(str(main_content), heading_style="ATX", strip=['a']).strip()
            print("\n" + "="*50)
            print("📄 EXTRACTED MARKDOWN CONTENT:")
            print("="*50 + "\n")
            print(clean_md)
        else:
            print("❌ Could not extract main content.")
    except Exception as e:
        print(f"❌ Error during scraping: {e}")
    finally:
        await page.close()

async def search_jobs(context, keywords, location="Worldwide"):
    page = await context.new_page()
    query = urllib.parse.urlencode({"keywords": keywords, "location": location, "f_TPR": "r86400"}) # r86400 = Past 24 hours
    search_url = f"https://www.linkedin.com/jobs/search/?{query}"
    
    print(f"🔍 Harvey Vision: Searching Jobs Wall -> {search_url}")
    try:
        await page.goto(search_url, wait_until="domcontentloaded", timeout=20000)
        await page.wait_for_timeout(5000) # Wait for results to load
        
        # Scroll down a bit to trigger lazy loading of the list
        await page.evaluate("window.scrollBy(0, document.body.scrollHeight)")
        await page.wait_for_timeout(2000)
        
        html_content = await page.content()
        soup = BeautifulSoup(html_content, "html.parser")
        
        job_cards = soup.find_all("div", {"data-job-id": True})
        
        print("\n" + "="*50)
        print(f"🎯 FOUND {len(job_cards)} RECENT JOBS FOR '{keywords}'")
        print("="*50 + "\n")
        
        for card in job_cards[:10]: # Limit to top 10 to avoid spam
            job_id = card.get("data-job-id")
            title_elem = card.find("a", {"class": "job-card-list__title"})
            company_elem = card.find("span", {"class": "job-card-container__primary-description"})
            
            title = title_elem.text.strip() if title_elem else "Unknown Title"
            company = company_elem.text.strip() if company_elem else "Unknown Company"
            link = f"https://www.linkedin.com/jobs/view/{job_id}/"
            
            print(f"🔹 {title} @ {company}")
            print(f"🔗 {link}\n")
            
    except Exception as e:
        print(f"❌ Error during job search: {e}")
    finally:
        await page.close()

async def main():
    if len(sys.argv) < 2:
        print("Usage:")
        print("  Read a job:   python3 linkedin_scraper.py read <linkedin_url>")
        print("  Search jobs:  python3 linkedin_scraper.py search <keywords> [location]")
        sys.exit(1)
        
    command = sys.argv[1]
    
    async with async_playwright() as p:
        browser, context = await get_browser_context(p)
        
        if command == "read" and len(sys.argv) >= 3:
            url = sys.argv[2]
            await scrape_job(context, url)
        elif command == "search" and len(sys.argv) >= 3:
            keywords = sys.argv[2]
            location = sys.argv[3] if len(sys.argv) > 3 else "Worldwide"
            await search_jobs(context, keywords, location)
        else:
            print("❌ Invalid command structure.")
            
        await browser.close()

if __name__ == "__main__":
    asyncio.run(main())
