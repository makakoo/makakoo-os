import json
import time
import os
from playwright.sync_api import sync_playwright

HARVEY_HOME = os.environ.get('HARVEY_HOME', os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
DB_PATH = os.path.join(HARVEY_HOME, 'data', 'career-manager', 'leads_data.json')

def parse_digests():
    with open(DB_PATH, 'r') as f:
        data = json.load(f)
        
    jobs = data if isinstance(data, list) else data.get('jobs', [])
    new_jobs = []
    parsed_count = 0
    
    print("🚀 Starting Digest Parser for XING and jobs.ch...")
    
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        state_file = os.path.join(HARVEY_HOME, "data", "linkedin-outreach", "state.json")
        
        try:
            if os.path.exists(state_file):
                context = browser.new_context(storage_state=state_file)
            else:
                context = browser.new_context()
        except:
            context = browser.new_context()
            
        page = context.new_page()
        
        # We will parse only up to 5 digests per run to not get banned
        digests_to_parse = []
        for job in jobs:
            if not isinstance(job, dict): continue
            if job.get('status') in ['Applied', 'Sent', 'Error', 'Rejected', 'Parsed']: continue
            
            url = job.get('url', job.get('link', ''))
            company = job.get('company', '').lower()
            
            if 'xing' in company or 'jobs.ch' in company or 'xing.com' in url or 'jobs.ch' in url:
                digests_to_parse.append(job)
                
        for job in digests_to_parse[:5]:
            url = job.get('url', job.get('link', ''))
            print(f"\n📦 Parsing digest: {job.get('title')}...")
            try:
                page.goto(url, timeout=15000)
                time.sleep(5) # wait for redirect and render
                
                extracted_this_round = 0
                
                # Close any annoying cookie banners
                try:
                    cookie_btn = page.locator("button:has-text('Accept'), button:has-text('Akzeptieren')").first
                    if cookie_btn.is_visible():
                        cookie_btn.click()
                        time.sleep(1)
                except:
                    pass
                
                if 'xing.com' in page.url:
                    # Extract XING jobs
                    links = page.locator('a').all()
                    
                    seen_urls = set()
                    for link in links:
                        href = link.get_attribute('href')
                        title = link.text_content().strip() if link.text_content() else ""
                        
                        title = title.replace('\n', ' ').strip()
                        
                        ignore_texts = ['view in browser', 'show all search results', 'actively looking', 'open to offers', 'not looking right now', 'privacy policy', 'unsubscribe', 'contact']
                        
                        if href and 'xing.com/m/' in href and len(title) > 10 and not any(ign in title.lower() for ign in ignore_texts):
                            clean_url = href.split('?')[0]
                            if clean_url in seen_urls: continue
                            seen_urls.add(clean_url)
                            
                            print(f"  -> Found: {title}")
                            new_jobs.append({
                                "title": title,
                                "company": "XING Extracted",
                                "url": href,
                                "match_score": job.get('match_score', job.get('score', 8)), 
                                "status": "Pending",
                                "source_digest": url
                            })
                            extracted_this_round += 1
                            
                elif 'jobs.ch' in page.url:
                    # Extract jobs.ch jobs
                    links = page.locator('a[data-cy="job-link"]').all()
                    
                    seen_urls = set()
                    for link in links:
                        href = link.get_attribute('href')
                        title = link.get_attribute('title')
                        if not title:
                             title = link.text_content().strip()
                        
                        title = title.replace('\n', ' ').strip()
                        
                        if href:
                            if href.startswith('/'):
                                href = 'https://www.jobs.ch' + href
                                
                            clean_url = href.split('?')[0]
                            if clean_url in seen_urls: continue
                            seen_urls.add(clean_url)
                                
                            print(f"  -> Found: {title}")
                            new_jobs.append({
                                "title": title,
                                "company": "jobs.ch Extracted",
                                "url": href,
                                "match_score": job.get('match_score', job.get('score', 8)),
                                "status": "Pending",
                                "source_digest": url
                            })
                            extracted_this_round += 1
                
                # Mark original digest as Parsed so we don't scan it again
                if extracted_this_round > 0:
                    job['status'] = 'Parsed'
                    parsed_count += 1
                else:
                    job['status'] = 'Error' # Mark error if we failed to parse anything
                    print("  -> Could not extract any jobs from this page.")
                
            except Exception as e:
                print(f"  -> Failed to parse: {e}")
                job['status'] = 'Error'
                
        browser.close()
        
    if new_jobs:
        print(f"\n🎉 Successfully extracted {len(new_jobs)} new individual jobs!")
        jobs.extend(new_jobs)
        
        if isinstance(data, dict):
            data['jobs'] = jobs
        else:
            data = jobs
            
        with open(DB_PATH, 'w') as f:
            json.dump(data, f, indent=2)
    else:
        print("\nNo new jobs extracted.")

if __name__ == "__main__":
    parse_digests()
