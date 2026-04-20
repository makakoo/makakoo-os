import asyncio
import os
import json
import urllib.parse
from playwright.async_api import async_playwright
from bs4 import BeautifulSoup

# Paths
HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
SESSION_FILE = os.path.join(HARVEY_HOME, "harvey-os", "skills", "career-manager", ".linkedin_session")
OUTPUT_FILE = os.path.join(HARVEY_HOME, "data", "career-manager", "broad_scraped_results.json")

async def search_linkedin(context, keyword):
    page = await context.new_page()
    params = {
        "keywords": keyword,
        "location": "Worldwide", # Go Worldwide for maximum volume
        "f_TPR": "r604800", # Past week for broader search
        "f_WT": "2" # Remote
    }
    url = f"https://www.linkedin.com/jobs/search/?{urllib.parse.urlencode(params)}"
    print(f"🔍 [LI] Searching: {keyword}")
    
    results = []
    try:
        await page.goto(url, wait_until="domcontentloaded", timeout=60000)
        await page.wait_for_timeout(10000)
        
        # Scroll more to load more jobs
        for _ in range(5):
            await page.evaluate("window.scrollBy(0, 1500)")
            await page.wait_for_timeout(2000)
            
        html = await page.content()
        soup = BeautifulSoup(html, "html.parser")
        
        links = soup.find_all("a", href=True)
        for link in links:
            href = link['href']
            if "/jobs/view/" in href:
                job_id = href.split("/view/")[1].split("/")[0]
                full_url = f"https://www.linkedin.com/jobs/view/{job_id}/"
                title = link.get_text(strip=True)
                if len(title) > 10:
                    results.append({
                        "title": title,
                        "company": "LinkedIn Company",
                        "url": full_url,
                        "source": "LinkedIn"
                    })
    except Exception as e:
        print(f"❌ LI Error ({keyword}): {e}")
    finally:
        await page.close()
    return results

async def search_jobsch(browser, keyword):
    page = await browser.new_page()
    # Broad search on jobs.ch
    url = f"https://www.jobs.ch/en/vacancies/?term={urllib.parse.quote(keyword)}"
    print(f"🔍 [CH] Searching: {keyword}")
    
    results = []
    try:
        await page.goto(url, wait_until="domcontentloaded", timeout=60000)
        await page.wait_for_timeout(5000)
        
        soup = BeautifulSoup(await page.content(), "html.parser")
        for a in soup.find_all("a", href=True):
            if "/vacancies/detail/" in a['href']:
                title = a.get_text(strip=True)
                if len(title) > 10:
                    all_text = a.parent.get_text() if a.parent else ""
                    # Check for freelance keywords in the item text
                    if any(k in all_text.lower() for k in ["contract", "freelance", "temporary", "project"]):
                        results.append({
                            "title": title,
                            "company": "jobs.ch Company",
                            "url": "https://www.jobs.ch" + a['href'] if a['href'].startswith('/') else a['href'],
                            "source": "jobs.ch"
                        })
    except Exception as e:
        print(f"❌ CH Error ({keyword}): {e}")
    finally:
        await page.close()
    return results

async def main():
    keywords = [
        "Ruby on Rails", "AI Agent", "Python AI", "Kubernetes", 
        "GCP Architect", "Rust Engineer", "Generative AI", 
        "Founding Engineer", "Senior Software Architect"
    ]
    all_results = []
    
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        li_at = open(SESSION_FILE).read().strip() if os.path.exists(SESSION_FILE) else ""
        context = await browser.new_context()
        if li_at:
            await context.add_cookies([{"name": "li_at", "value": li_at, "domain": ".www.linkedin.com", "path": "/"}])
        
        for kw in keywords:
            # Run LinkedIn and jobs.ch for each
            tasks = [search_linkedin(context, kw), search_jobsch(browser, kw)]
            results_list = await asyncio.gather(*tasks)
            for res in results_list:
                all_results.extend(res)
            
        await browser.close()
        
    print(f"\n✅ Broad search complete. Total raw jobs: {len(all_results)}")
    
    # Filter duplicates
    unique_results = []
    seen_urls = set()
    for res in all_results:
        clean_url = res['url'].split('?')[0].rstrip('/')
        if clean_url not in seen_urls:
            seen_urls.add(clean_url)
            unique_results.append(res)
            
    with open(OUTPUT_FILE, "w") as f:
        json.dump(unique_results, f, indent=2)
    
    print(f"✅ Unique jobs saved: {len(unique_results)}")

if __name__ == "__main__":
    asyncio.run(main())
