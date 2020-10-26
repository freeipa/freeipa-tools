#!/usr/bin/python3
# Authors:
#   Martin Kosek <mkosek@redhat.com>
#
# Copyright (C) 2011  Red Hat
# see file 'COPYING' for use and warranty information
#
# This program is free software; you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <http://www.gnu.org/licenses/>.

import subprocess
import re
import os
import shutil
from contextlib import contextmanager
import tempfile
import icu      # sudo dnf install pyicu

@contextmanager
def chdir(path):
    old_dir = os.getcwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(old_dir)

def contributors_from_git(git_directory, branch):
    git_authors = []
    with chdir(git_directory):
        cmd = ['git', 'log', '--format=format:%aN', '--use-mailmap']
        git_authors = subprocess.check_output(cmd, stderr=subprocess.STDOUT, encoding='UTF-8').splitlines()
    return set(git_authors)

def contributors_from_po(git_directory, branch):
    po_authors = []
    with chdir(git_directory):
        cmd = ['git', 'grep', '-h', '#zanata', 'po/']
        po_attrs = subprocess.check_output(cmd, stderr=subprocess.STDOUT, encoding='UTF-8').splitlines()
        for author in po_attrs:
            po_authors.append(" ".join([x for x in author.split() if x[0] != '#' and x[0] != '<' and not x[0].isdigit()]))
    return set(po_authors)

def update_changelog(git_directory, branch):
    git_authors = contributors_from_git(git_directory, branch)
    po_authors = contributors_from_po(git_directory, branch)

    contributors_file_path = os.path.join(git_directory, 'Contributors.txt')
    with open(contributors_file_path, 'r') as f:
        lines = f.readlines()
        new_file = []
        in_developers = False
        in_translators = False
        file_authors = set()
        for line in lines:
            if line.startswith("Developers:"):
                in_developers = True
                new_file.append(line)
                continue

            if line.startswith("Translators:"):
                in_translators = True
                new_file.append(line)
                continue

            if (in_developers or in_translators) and not line.strip():
                # last developer
                if in_developers:
                    in_developers = False
                    authors = list(git_authors | file_authors)
                if in_translators:
                    in_translators = False
                    authors = list(po_authors | file_authors)
                collator = icu.Collator.createInstance(icu.Locale('en_US.UTF-8'))
                authors.sort(key=collator.getSortKey)

                for author in authors:
                    new_file.append(u"\t%s\n" % author)

                file_authors = set()

            if in_developers or in_translators:
                author = line.strip()
                file_authors.add(author)
            else:
                new_file.append(line)

        (update_fd, update_file) = tempfile.mkstemp()
        update_file_content = u"".join(new_file).encode("UTF-8")
        os.write(update_fd, update_file_content)
        os.close(update_fd)
        shutil.move(update_file, contributors_file_path)

def main():
    # Update changelog in current working directory master branch
    path = os.getcwd()
    update_changelog(path, "HEAD")

if __name__ == "__main__":
    main()
