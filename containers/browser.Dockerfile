FROM mcr.microsoft.com/playwright/python:v1.56.0-noble@sha256:a7f6cf3ae520c9d670ad956572c13747ed5abdbba5123a01526f873ed1662528
RUN pip install --no-cache-dir playwright==1.56.0
COPY tools/browser_smoke.py tools/smoke_release.py tools/material_browser.py tools/admin_ruleset_browser.py tools/catalogue_browser.py tools/assist_demo.py tools/assist_model_browser.py tools/review_browser.py tools/upload_browser.py tools/suggest_browser.py tools/master_browser.py /checks/
COPY tools/fixtures/upload-three-pages.pdf.txt /checks/fixtures/
ENTRYPOINT ["python", "/checks/browser_smoke.py"]
