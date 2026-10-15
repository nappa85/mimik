FROM mcr.microsoft.com/playwright/python:v1.56.0-noble@sha256:a7f6cf3ae520c9d670ad956572c13747ed5abdbba5123a01526f873ed1662528
RUN pip install --no-cache-dir playwright==1.56.0
COPY tools/browser_smoke.py tools/smoke_release.py tools/material_browser.py /checks/
ENTRYPOINT ["python", "/checks/browser_smoke.py"]
