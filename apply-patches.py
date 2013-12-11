#! /usr/bin/env python2

""" Apply patches with leading ">" stripped

Usage: apply-patches.py patchdir/*.patch
"""

import sys
import subprocess

filenames = sys.argv[1:]

for filename in filenames:
    print '*', filename
    git_am = subprocess.Popen(['git', 'am'], stdin=subprocess.PIPE)
    with open(filename) as file:
        git_am.stdin.write(file.readline().lstrip('>'))
        for line in file:
            git_am.stdin.write(line)
    git_am.communicate()
