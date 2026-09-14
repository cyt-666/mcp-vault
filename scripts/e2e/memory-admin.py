#!/usr/bin/env python3
"""Real Chromium + disposable Admin HTTP + synthetic Provider. No production input."""
import json, os, subprocess, tempfile, time
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

root=Path(__file__).resolve().parents[2]
output=Path(os.environ.get('MCP_VAULT_E2E_OUTPUT_DIR',str(root/'target/memory-review/browser-e2e')))
output.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(prefix='mcp-vault-browser-') as temporary:
    manifest=Path(temporary)/'manifest.json'
    environment=dict(os.environ,MCP_VAULT_FIXTURE_MANIFEST=str(manifest),MCP_VAULT_FIXTURE_MEMORY_ADMIN='1')
    with (output/'fixture.log').open('w') as log:
        fixture=subprocess.Popen([str(root/'target/debug/mcp-vault-fixture')],cwd=root,env=environment,stdout=log,stderr=subprocess.STDOUT)
        try:
            for _ in range(1200):
                if manifest.exists(): break
                if fixture.poll() is not None: raise RuntimeError('fixture exited; inspect fixture.log')
                time.sleep(.1)
            config=json.loads(manifest.read_text())
            origin=config['admin_url'].split('/api/')[0]
            with sync_playwright() as p:
                browser=p.chromium.launch(headless=True,executable_path=os.environ.get("MCP_VAULT_BROWSER_EXECUTABLE"))
                page=browser.new_page()
                requests=[]
                page.on('request',lambda request: requests.append((request.method,request.url)) if '/api/v1/' in request.url else None)
                page.on('dialog',lambda dialog:dialog.accept())
                page.set_default_timeout(15000)
                try:
                    page.goto(origin)
                    page.get_by_label('用户名',exact=True).fill('browser-admin')
                    page.get_by_label('密码',exact=True).fill('isolated-browser-password-123')
                    # Let the fixture's initial outbox/embedding jobs drain before login.
                    page.wait_for_timeout(2000)
                    for login_attempt in range(3):
                        page.get_by_role('button',name='登录管理端',exact=True).click()
                        page.wait_for_timeout(1000)
                        if page.get_by_role('button',name='记忆',exact=True).count(): break
                        if not page.get_by_text('请求失败（authentication_unavailable），请稍后重试。',exact=True).count(): break

                    page.get_by_role('button',name='记忆',exact=True).first.click()
                    expect(page.get_by_role('heading',name='记忆生成与概览',exact=True)).to_be_visible()
                    expect(page.get_by_role('heading',name='记忆概览',exact=True)).to_be_visible()
                    for retired in ['迁移预检','运行／重试评测','立即整理']:
                        expect(page.get_by_role('button',name=retired,exact=True)).to_have_count(0)
                    expect(page.get_by_text('当前记忆向量覆盖完整，可用于语义检索；相似度不代表资料一定回答了问题。',exact=True)).to_be_visible()
                    # Real server pagination and explicit revision-aware edit.
                    page.get_by_role('button',name='加载更多记忆',exact=True).click()
                    expect(page.get_by_text('长期记忆（已加载 53 条）',exact=True)).to_be_visible()
                    page.get_by_role('button',name='编辑显式记忆',exact=False).first.click(force=True)
                    edited='  Browser edited explicit assertion\n  Preserve these spaces.\n'
                    page.locator('form[aria-label="编辑显式记忆"] textarea').fill(edited)
                    page.get_by_role('button',name='保存修改',exact=True).click()
                    expect(page.locator('strong.memory-content').filter(has_text='Browser edited explicit assertion')).to_have_text(edited)
                    assert any(method=='PATCH' and '/memories/' in url for method,url in requests)
                    # Exact complete source body, source navigation and independently owned copy.
                    body='# Browser source\nThe synthetic team completed its local exercise.'
                    item=page.locator('article.record-item').filter(has=page.get_by_text('笔记派生',exact=True)).filter(has_text='The synthetic team completed its local exercise.').last
                    item.get_by_text('查看来源笔记与证据定位（1）',exact=True).click()
                    page.get_by_text('在本机 Obsidian 打开来源',exact=True).click()
                    page.get_by_label('本机 Vault 名称',exact=True).fill('Browser Vault 中文')
                    link=item.get_by_role('link',name='打开来源：browser-source.md',exact=True)
                    assert link.get_attribute('href')=='obsidian://open?vault=Browser%20Vault%20%E4%B8%AD%E6%96%87&file=browser-source.md%23Browser%20source'
                    item.get_by_role('button',name='另存为明确记忆',exact=True).click()
                    expect(page.get_by_role('textbox',name='内容',exact=True)).to_have_value(body)
                    page.get_by_role('button',name='保存当前记忆',exact=True).click()
                    expect(page.get_by_text('显式记忆已直接保存，不需要模型整理。',exact=True)).to_be_visible()
                    explicit_copy=page.locator('article.record-item').filter(has=page.get_by_text('显式',exact=True)).filter(has_text='The synthetic team completed its local exercise.')
                    expect(explicit_copy).to_have_count(1)
                    explicit_copy.get_by_text('查看来源笔记与证据定位（1）',exact=True).click()
                    expect(explicit_copy.get_by_role('link',name='打开来源：browser-source.md',exact=True)).to_be_visible()
                    # Deletion pauses the source, while the saved explicit unit survives.
                    item.get_by_role('button',name='删除当前记忆',exact=False).click()
                    page.get_by_role('button',name='刷新状态',exact=True).click()
                    expect(page.get_by_role('button',name='恢复来源提取',exact=True)).to_be_visible()
                    expect(explicit_copy).to_have_count(1)
                    page.get_by_role('button',name='恢复来源提取',exact=True).click()
                    expect(page.get_by_text('来源恢复已提交，后台完成后自动提取。',exact=True)).to_be_visible()
                    # Directory filtering and full-unit reads operate through real HTTP.
                    page.get_by_label('目录路径（可选）',exact=True).fill('missing-directory')
                    page.get_by_role('button',name='查看目录',exact=True).click()
                    overview=page.locator('article.panel').filter(has=page.get_by_role('heading',name='记忆概览',exact=True))
                    expect(overview.get_by_role('button',name='读取完整记忆',exact=True)).to_have_count(0)
                    page.get_by_label('目录路径（可选）',exact=True).fill('')
                    page.get_by_role('button',name='查看目录',exact=True).click()
                    overview.get_by_role('button',name='读取完整记忆',exact=True).first.click()
                    expect(overview.get_by_text('完整记忆',exact=True)).to_be_visible()
                    overview.get_by_role('button',name='收起',exact=True).click()
                    # Generation control uses new endpoints, preserving durable progress.
                    page.get_by_role('button',name='暂停生成',exact=True).click()
                    expect(page.get_by_role('button',name='继续生成',exact=True)).to_be_visible()
                    page.get_by_role('button',name='继续生成',exact=True).click()
                    expect(page.get_by_role('button',name='处理来源与概览',exact=True)).to_be_visible()
                    assert any(method=='POST' and url.endswith('/memory/generation') for method,url in requests)
                    assert not any('/semantic-calibration' in url or '/memory/organization' in url or '/memory/migration' in url for _,url in requests)
                    assert page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), 'memory page must not overflow horizontally'
                    page.screenshot(path=str(output/'memory-admin.png'),full_page=True)
                    overview.screenshot(path=str(output/'overview-panel.png'))
                    page.get_by_role('button',name='AI 服务',exact=True).first.click()
                    page.get_by_text('高级：摘要、Embedding 与重排模型',exact=True).click()
                    expect(page.get_by_text('记忆概览（可选）',exact=True)).to_be_visible()
                    expect(page.get_by_text('笔记语义分块（可选）',exact=True)).to_have_count(0)
                    (output/'result.json').write_text(json.dumps({'result':'passed','backend':'real_admin_http','provider':'local_synthetic_contract','flows':['generation_pause_resume','exact_explicit_edit','copy_with_provenance','source_delete_resume','pagination','scoped_overview_full_read','obsidian_encoded_source_link','retired_routes_absent'],'production_data':False},indent=2)+'\n')
                except Exception:
                    (output/'failure.txt').write_text(page.locator('body').inner_text())
                    page.screenshot(path=str(output/'failure.png'),full_page=True,timeout=10000)
                    raise
                browser.close()
                print('PASS: real browser v3 memory flows; local contract Provider only')
        finally:
            fixture.terminate()
            try: fixture.wait(timeout=10)
            except subprocess.TimeoutExpired: fixture.kill();fixture.wait()
