
import sqlalchemy as _sa

def statement(options, plugin):
    return str(list(_sa.create_engine(options["engine"]).execute(options["statement"]))[0][0])
