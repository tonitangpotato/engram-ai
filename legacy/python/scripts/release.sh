#!/bin/bash
# Release script for Engram v1.0.0

set -e

VERSION="1.0.0"
echo "🚀 Releasing Engram v${VERSION}"
echo ""

# Check if on main branch
BRANCH=$(git branch --show-current)
if [ "$BRANCH" != "main" ]; then
  echo "❌ Must be on main branch (currently on: $BRANCH)"
  exit 1
fi

# Check for uncommitted changes
if [ -n "$(git status --porcelain)" ]; then
  echo "⚠️  Uncommitted changes detected:"
  git status --short
  read -p "Continue anyway? (y/N) " -n 1 -r
  echo
  if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    exit 1
  fi
fi

echo "1️⃣  Cleaning old builds..."
rm -rf dist/ build/ *.egg-info
echo "✅ Cleaned"
echo ""

echo "2️⃣  Building distribution packages..."
python3 -m pip install --upgrade build twine
python3 -m build
echo "✅ Built"
echo ""

echo "3️⃣  Checking package..."
python3 -m twine check dist/*
echo "✅ Package valid"
echo ""

echo "📦 Package contents:"
ls -lh dist/
echo ""

echo "4️⃣  Ready to upload to PyPI"
echo ""
echo "To upload to PyPI:"
echo "  python3 -m twine upload dist/*"
echo ""
echo "To test upload first (TestPyPI):"
echo "  python3 -m twine upload --repository testpypi dist/*"
echo ""

read -p "Upload to PyPI now? (y/N) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
  echo "📤 Uploading to PyPI..."
  python3 -m twine upload dist/*
  echo "✅ Published to PyPI!"
  echo ""
  
  echo "5️⃣  Tagging release..."
  git tag -a "v${VERSION}" -m "Release v${VERSION}: Semantic embedding + auto-fallback (production-ready)"
  git push origin "v${VERSION}"
  echo "✅ Tagged v${VERSION}"
  echo ""
  
  echo "🎉 Release complete!"
  echo ""
  echo "Next steps:"
  echo "  1. Create GitHub release: https://github.com/tonitangpotato/engram-ai/releases/new"
  echo "  2. Update BotCore: cd ../botcore && npm version patch && npm publish"
  echo "  3. Announce on Discord: https://discord.com/invite/clawd"
else
  echo "⏸️  Skipped upload"
  echo ""
  echo "To upload later:"
  echo "  python3 -m twine upload dist/*"
fi
